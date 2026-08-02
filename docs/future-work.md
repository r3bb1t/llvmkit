# Future work

The live backlog: what is known-missing, what was deliberately deferred, and
**why** in each case. Several rustdoc comments in `crates/` point here rather
than restating a deferral inline, so entries are written to be read cold.

Each item cites its source — a file and symbol, an upstream reference, or the
cycle that decided it. Items that later shipped are struck through and dated
rather than deleted, so a reader can tell "never done" from "done, here is what
actually landed".

It began as the residue of the `feature-1/irbuilder-type-safety` audits and has
accumulated every cycle since; the oldest sections are still organised that way.

## ~~Parser — deferred alias/ifunc targets~~ (found and fixed 2026-07-31)

~~The printer emits aliases and ifuncs before function declarations, but the
parser resolved their target eagerly, so a printed module whose ifunc resolver
was declared later did not re-parse.~~ **Fixed:** forward targets become a null
placeholder patched at end of module, mirroring `personality`. Covered by
`parser_attribute_matrix.rs::alias_and_ifunc_forward_targets`.

## ~~Parser — lexer diagnostics carry no text~~ (found 2026-07-31, fixed 2026-07-31)

~~`LexError::UnknownToken` renders as a bare `invalid token`, so an unknown
attribute keyword, a bogus global property, and a malformed `uwtable(...)` kind
all produce the same uninformative message.~~ **Fixed** in
`feature-39/lexer-diagnostics`: the variant gained a
`reason: UnknownTokenReason` payload and every one of the ten construction
sites names its own failure — `unknown keyword 'nocalback'`, `no token starts
with '\x01'`, `expected a comdat name after '$'`, `expected hexadecimal digits
after '0xK'`, and so on. `LexError::UnknownToken` stays a single variant on
purpose: the parser genuinely uses it as the category "the lexer could not form
a token here, let me supply production context", and splitting it would have
forced both re-mapping sites to enumerate every reason.

Two things worth knowing for anyone extending it:

- **There is no upstream message to port.** `LLLexer` returns a bare
  `lltok::Error` at all of these sites and records nothing
  (every `return lltok::Error` in `LLLexer`); `LLParser` supplies
  the wording from the surrounding production. That is adequate when the parser
  is always the caller, which is true for `llvm-as` and false for llvmkit,
  whose lexer is public. So these messages are a deliberate improvement on
  upstream rather than a port, and the tests say so.
- **The unknown-keyword span was widened to the whole word** while the cursor
  rewind (`self.pos = tok_start + 1`, upstream's `LLLexer::LexIdentifier`
  behaviour)
  was kept exactly. A caret under the `n` of `nocalback` helps nobody. Lexing
  behaviour is unchanged; only the reported span moved.

## ~~ApFloat / `ApInt` bit-exactness audit~~ — closed (2026-08-01)

Both halves are **complete**. Every `APFloatTest.cpp` family covering the seven
modeled semantics and every `APIntTest.cpp` family llvmkit can express is
ported, and the fourteen defects they found are fixed. `Float8*` / `Float6*` /
`Float4*` / `TF32` stay out of scope — llvmkit does not model those semantics.

What is deliberately **not** ported from `APIntTest.cpp`, and why:

| Family | Why |
|---|---|
| `rvalue_arithmetic`, `rvalue_bitwise`, `rvalue_invert`, `SelfMoveAssignment` | C++ move-semantics fixtures; Rust move semantics make the property they check unrepresentable. |
| `tcDecrement` | The `tc*` word-level primitives are upstream's internal API. llvmkit stores words in a `Box<[u64]>` and exposes no equivalent surface, by design. |
| `StringBitsNeeded2` / `8` / `10` / `16`, `StringDeath` | `APInt::getBitsNeeded` sizes a width from a string. `ApInt::from_string` always takes an explicit width, so nothing calls it; `StringDeath` is an assertion-death test. |
| `mul_clear` | Checks that `operator*=` clears unused bits the same way `operator*` does. llvmkit has one `wrapping_mul`, so the two paths it compares are the same path. |
| `LargeAPIntConstruction` | Builds `APInt(UINT32_MAX, 0)` — a four-billion-bit value that allocates half a gigabyte. The behavior it guards (no crash on a legal but absurd width) is structural here. |
| `isAligned` | Needs upstream's `Align` type, which llvmkit does not model on `ApInt`. |
| `binaryOpsWithRawIntegers` | Upstream's scalar *arithmetic* overloads (`APInt + uint64_t`). llvmkit deliberately requires both operands to name a width; the comparison half of that story is covered by `unsigned_cmp_u64` / `signed_cmp_i64`. |
| `SolveQuadraticEquationWrap` | SCEV-specific; belongs with a SCEV port, not with `ApInt`. |
| `GetMostSignificantDifferentBitExaustive` | The non-exhaustive fixture is ported; the exhaustive variant re-derives the same property over an 8-bit sweep. |

## ValueTracking.h — the road to 100% (measured 2026-08-01)

`KnownBits.h` is fully modeled. `ValueTracking.h` is not, and the honest
reason is that its unmodeled entry points are blocked on **missing types**,
not on missing effort. Measured against the vendored 22.1.4 tree:

| Prerequisite | Upstream size | llvmkit today | Gap |
|---|---|---|---|
| `ConstantRange` | 632 (`.h`) + 2314 (`.cpp`) | `constant_range.rs`, 223 lines, **15 of 78** public methods | ~63 methods + `ConstantRangeTest.cpp` (~2800 lines) |
| `KnownFPClass` / `FPClassTest` | `FloatingPointMode.h` 290, plus ~1500 lines of `computeKnownFPClass` | **absent** | a whole FP lattice |
| `AssumptionCache` | 280 + 310 | **absent** | also needs `@llvm.assume` modeling |
| `SelectPatternResult` | declared in `ValueTracking.h` | **absent** | ~250 lines |
| `TargetLibraryInfo` | 664 | `target_library_info.rs`, 427 lines | partial; may already suffice |
| `ValueTracking.cpp` itself | 10535 | `value_tracking.rs`, 2259 | ~8300 lines |

Roughly **12–14k lines of ported logic**, plus D11-compliant ported tests for
each (upstream's `ValueTrackingTest.cpp` is ~5000 lines). Comparable in size to
the whole ApFloat/ApInt sweep, which was its own multi-cycle program.

**Suggested tranche order.** Tranche 1 needs no new types at all and unblocks
the most callers, so it should go first regardless of how the rest is
sequenced:

1. **No new types** — `ComputeNumSignBits`, `ComputeMaxSignificantBits`,
   `isKnownNegative` / `isKnownPositive` / `isKnownNonNegative`,
   `isKnownToBeAPowerOfTwo`, `isKnownNonEqual`, `isKnownInversion`,
   `isKnownNegation`, the Value-level `haveNoCommonBitsSet`. Pure KnownBits
   consumers plus recursion.
2. **Poison / UB family** — `canCreatePoison`, `canCreateUndefOrPoison`,
   `impliesPoison`, `propagatesPoison`, `programUndefinedIfPoison`,
   `mustTriggerUB`, `isGuaranteedNotToBeUndef`. llvmkit already has
   `is_guaranteed_not_to_be_poison` internally, so this extends an existing
   seed. Still no new types.
3. ~~**`ConstantRange` to completion**, then the families it gates.~~
   **DONE 2026-08-02** — all five slices. `ConstantRange` went from 13 of 83
   public methods to 90 llvmkit methods covering all but `castOp`,
   `shlWithNoWrap` and the signedness-flipping helpers, each recorded below.
   Cut into five sub-slices, each mergeable on its own (2026-08-01). llvmkit
   maps 13 of the 78 public methods today; the count below is what each slice
   adds.

   | Slice | Adds | Content |
   |---|---|---|
   | **3a** | ~16 | Bounds and predicates: `getSignedMax`/`Min`, `isSignWrappedSet`, `isUpperSignWrapped`, `isSingleElement`, `isAllNegative`/`isAllNonNegative`/`isAllPositive`, `isSizeLargerThan`, `isSizeStrictlySmallerThan`, `getActiveBits`, `getMinSignedBits`, `getNonEmpty`, `toKnownBits`, `fromKnownBits`. No arithmetic; exhaustively testable at 4 bits. |
   | ~~**3b**~~ | ~12 | ~~Set operations~~ **done 2026-08-02**: `intersect_with`, `union_with`, `difference`, `subtract`, `inverse`, `split_pos_neg`, `zero_extend`, `sign_extend`, `truncate`, `zext_or_trunc`, `sext_or_trunc`. `castOp` is **not** ported — it dispatches on `Instruction::CastOps` over the ten cast opcodes, but eight of them are no-ops or bail to full; the two that matter (`zext`, `sext`, `trunc`) are the methods above, and a caller with a `CastOpcode` in hand can match on it directly. Revisit if a consumer wants the dispatcher itself. |
   | ~~**3c**~~ | ~9 | ~~ICmp regions~~ **done 2026-08-02**: `make_allowed_icmp_region`, `make_satisfying_icmp_region`, `make_exact_icmp_region`, `make_mask_not_equal_range`, `equivalent_icmp` / `equivalent_icmp_with_offset`, `icmp`, plus the `single` and `contains_range` constructors/queries they needed. The signedness-flipping helpers (`areInsensitiveToSignednessOfICmpPredicate`, `getEquivalentPredWithFlippedSignedness`) are **not** ported — nothing in llvmkit calls them, and they exist upstream for InstCombine's predicate canonicalization, which llvmkit does not have. |
   | ~~**3d**~~ ✅ | ~37 | **done 2026-08-02** across six sub-slices: |
   | 3d-i ✅ | 7 | **done 2026-08-02**: `add`, `sub`, `multiply`, `smax`/`smin`/`umax`/`umin`. These are exactly what `computeOverflowFor*` in 3e needs, which is why they went first. |
   | 3d-ii ✅ | 5 | **done 2026-08-02**: `udiv`, `sdiv`, `urem`, `srem`, plus `abs` pulled forward from 3d-v because `srem` needs it. |
   | 3d-iii ✅ | 7 | **done 2026-08-02**: `binary_and`, `binary_or`, `binary_xor`, `binary_not`, `shl`, `lshr`, `ashr`. Found and fixed a real `ApInt::ashr` bug on the way — see the CHANGELOG. |
   | 3d-iv ✅ | 8 | **done 2026-08-02**: the saturating family. Six share one frame (`saturating_pairwise`); `smul_sat` needs all four corners and `sshl_sat` picks its shift by endpoint sign. |
   | 3d-v ✅ | 3 | **done 2026-08-02**: `ctlz`, `cttz`, `ctpop` (`abs` landed in 3d-ii). |
   | 3d-vi ✅ | 8 | **done 2026-08-02**: `binary_op`, `overflowing_binary_op`, `intrinsic`, `is_intrinsic_supported`, `add_with_no_wrap`, `sub_with_no_wrap`, `multiply_with_no_wrap`, `smul_fast`. **`shlWithNoWrap` is not ported** — it is three helper functions (`computeShlNUW`, `computeShlNSWWithNNegLHS`, `computeShlNSWWithNegLHS`) plus a dispatcher, and no llvmkit caller needs it yet; `overflowing_binary_op` sends `shl` to the plain `shl`, which is sound and only weaker. |
   | ~~**3e**~~ ✅ | 8 | **done 2026-08-02**: `compute_constant_range`, `compute_constant_range_including_known_bits`, and all six `compute_overflow_for_*`. Also added the five `ConstantRange` overflow predicates (`unsigned_add_may_overflow` and siblings) that an earlier count missed — the real public surface is **83**, not 78; those five span two lines in the header and the extraction grep skipped them. **`getVScaleRange` is not ported**: it reads `vscale_range`'s packed `(min, max)` pair, and that attribute is already on `attribute_td_drift.rs`'s `NOT_YET_MODELED` list with a single-`u64` payload here. Porting it would mean inventing the second half. |

   `ConstantRangeTest.cpp` (~2800 lines) is the test source throughout; its
   exhaustive-over-4-bit-ranges harness ports directly and is the right
   oracle for every slice.
4. **`SelectPatternResult`** — `getSelectPattern`,
   `matchDecomposedSelectPattern`, `getMinMaxIntrinsic`,
   `getInverseMinMaxIntrinsic`.
5. **Pointer / object analysis** — `getUnderlyingObjects`,
   `getConstantStringInfo`, `GetStringLength`, `onlyUsedByLifetimeMarkers`.
6. **Speculation safety** — `isSafeToSpeculativelyExecute*`,
   `isGuaranteedToTransferExecutionToSuccessor`, `isValidAssumeForContext`.
7. **`KnownFPClass`** — the largest single piece, and the only one that needs
   a new lattice rather than a new container.
8. **`AssumptionCache`** — needs `@llvm.assume` modeled first;
   `computeKnownBitsFromContext` depends on it.

The ledger in `crates/llvmkit-ir/tests/value_tracking_parity.rs` tracks
progress: each tranche moves rows from `VALUE_TRACKING_GAPS` into
`MODELED_VALUE_TRACKING`, and the modeled column is held to the crate by
calling every entry.

## ~~KnownBits — operations not modeled~~ — closed (2026-08-01)

`KnownBits.h`'s **public** surface is now fully modeled. The ledger
(`crates/llvmkit-ir/tests/value_tracking_parity.rs`) asserts an empty gap list,
so a regression or a newly-synced upstream method has to be acknowledged rather
than absorbed.

Three were implemented to close it: `setAllOnes` → `set_all_ones`,
`isSignUnknown` → `is_sign_unknown`, and the plain two-argument `sdiv`
alongside the existing `sdiv_with_exact` (upstream spells the pair as one
function with `Exact = false` defaulted; llvmkit spells it as a pair, matching
`udiv` / `udiv_with_exact`).

Two entries an earlier revision of the ledger listed as gaps were **wrong in
both directions**: `flipSignBit` and `remGetLowBits` are declared outside
`public:` in `KnownBits.h`, and both already existed in llvmkit as
module-private free functions in `known_bits.rs`. They are neither public
upstream nor absent here, and the ledger now records them as such.

## KnownBits / ValueTracking — the PHI recurrence arm (2026-08-01)

`computeKnownBitsFromOperator`'s `Instruction::PHI` arm is ported, including
`matchSimpleRecurrence`. `llvm/test/Analysis/ValueTracking/recurrence-knownbits.ll`
is checked in verbatim and driven by
`crates/llvmkit-asmparser/tests/value_tracking_recurrence.rs`; twelve of its
fifteen functions reproduce their CHECK line exactly.

**Three do not, and both reasons are missing transforms rather than missing
analysis.** They are asserted to stay *unfolded*, so closing either gap trips
the test rather than passing silently.

| Function | Upstream CHECK | Why it is out of reach |
|---|---|---|
| `@test_mul` | `0` | Needs bit 1 of `%iv` known zero. The `mul` arm keeps only `min(countMinTrailingZeros(8), countMinTrailingZeros(2))` = 1 trailing zero. Upstream first canonicalizes `mul i64 %iv, 2` to `shl i64 %iv, 1`, and the *shift* arm then keeps all three trailing zeros of the start value — `@test_shl` is that same recurrence pre-canonicalized, and it does reach its CHECK. Closes when InstCombine's mul-by-power-of-two canonicalization runs. |
| `@test_and` | `2047` | Needs bits 11..63 known zero *and* bit 10 known **one**. The `and`/`or` arm only ever sets low zero bits, and `min(countMinTrailingZeros(1025), countMinTrailingZeros(1024))` is 0; the fallthrough intersection leaves bit 10 unknown. Upstream gets `2047` by simplifying the loop away, not from known bits. |
| `@test_or` | `2047` | As `@test_and`. The intersection *does* prove bit 10 is one here, but bits 11..63 stay unknown. |

Two further pieces of the arm are **not** ported because llvmkit does not model
what they read — the per-edge context instruction
(`RecQ.CxtI = P->getIncomingBlock(..)`) and the `m_Br(m_c_ICmp(..))` refinement
that narrows an incoming value by the branch condition guarding its edge.
Neither can make an answer wrong; each only leaves it weaker.

One divergence is **deliberate**. Upstream gates the incoming-value
intersection on `Depth < MaxAnalysisRecursionDepth - 1` and recurses at that
fixed depth, capping the search at one level. llvmkit recurses at `depth + 1`,
because it already terminates by a different mechanism (the `stack` set rejects
re-entering a value mid-computation) and because `compute_known_bits_inner`
memoizes on `(slot, query)` with no depth component — entering at a fixed deep
depth would cache the weak answer computed there and hand it to a later shallow
query. llvmkit therefore answers *more* precisely than upstream for a shallow
phi, never less. `@test_udiv_neg` witnesses it: llvmkit proves 60 leading zeros
where upstream proves none, and the fixture's own claim (bit 2 unknown) is
untouched. **If the cache ever becomes depth-keyed, revisit this** — the
upstream cap would then be portable as-is.

The `nsw` sign-inference paths of the `add`/`sub`/`mul` arm
(`makeNonNegative` / `makeNegative`) have **no ported fixture**: every
recurrence in `recurrence-knownbits.ll` uses `nuw` or no flag at all. Worth a
sweep for an upstream fixture that exercises them.

## Bare brands / `Branded` derive — home and follow-ups (2026-07-31)

- **`llvmkit-macros` is the permanent home of the `Branded` derive, not a
  stopgap.** Upstream LLVM's answer to drift-prone repetitive definitions is
  build-time generation (its lexer and parser both `#include` the
  TableGen-generated `Attributes.inc` — `LLLexer.cpp:701-704`,
  `LLParser.cpp:1547-1551` in the vendored 22.1.4 tree); `gen_intrinsics` in
  `build.rs` and the macros crate are this project's two arms of the same
  philosophy. The ecosystem norm agrees (`serde_derive`, `thiserror-impl` are
  permanent companion crates).
- **Optional simplification, not planned work:** RFC 3698 (`macro_derive`,
  rust-lang/rust#143549) would let the derive live inside `llvmkit-ir` as a
  `macro_rules!` derive. It is nightly-only with open stabilization blockers.
  If it ever stabilizes at or below the pinned toolchain, the migration is one
  re-export line (`lib.rs`'s `pub(crate) use llvmkit_macros::Branded`) — worth
  doing only if `llvmkit-macros` optionality matters then.
- **Fold backlog: ~30 hand-written bound-free impl families remain** (≈150
  impls) that predate the derive and could migrate to `#[derive(Branded)]`
  one family at a time: `Type`, `Value`, `IntValue`, `FloatValue`,
  `ArrayValue`, `VectorValue`, `ModuleRef`, `ModuleView`, `FunctionValue`,
  `CallInst`/`TypedCallInst`, the phi handles, `SwitchInst`/`IndirectBrInst`/
  `InvokeInst`/`LandingPadInst`/`CatchSwitchInst`, `IntType`/`FloatType`/
  `ArrayType`/`StructType`/`VectorType`, `TypedPointerValue`, `SsaBlock`, the
  SSA variables, `MetadataId`, `Instruction`. Each fold must compare the
  hand-written `PartialEq`/`Hash` field walk against the full field list and
  keep custom `Debug` bodies manual (several print computed values). Sound as
  they are; this is deduplication, not a fix.

## 0.0.4 program plan — deviation records (2026-07-31)

A post-freeze verification pass compared the shipped library against the
2026-07-24 id-first program plan. Four deviations; one resolved in code (the
supertrait drop above), three resolved by record — reality wins:

1. **`BasicBlockLabel` lives by design.** The plan's law 6 said "deleted",
   contradicting law 1 ("every handle survives as the view layer"). The
   implementation resolved the contradiction correctly: `BlockId` is the
   storable id, `BasicBlockLabel` the view it resolves to.
2. **The tag check lives at the id→slot boundary, not inside the arena
   accessors.** Plan invariant 5 predates A1's decision to make slots bare
   untagged indices; once slots carry no tag, `Context::value_data` has
   nothing to check. The check sits in `into_stored` / `resolve_in`.
   Verified unbypassable: public methods *return* raw slots (19 of them),
   none accept one, none construct one.
3. **`ValueSlot`/`TypeSlot` stay `pub`** — `llvmkit-asmparser` genuinely
   consumes them. Narrowing that surface (the parser carrying tagged ids
   instead) is folded into the Milestone 0 parser cycle, which reworks that
   crate anyway.

## Killer-feature designs (deferred)

- **Inline IR macro DSL** -- a `ir!{ %sum = add i32 %a, %b }` proc-macro
  added to the EXISTING `crates/llvmkit-macros/` crate (which already ships
  the `IrStruct` derive in `ir_struct.rs`; new sibling module `ir.rs` per the
  one-concept-per-file convention). Expands `.ll`-flavored syntax into typed
  builder calls at compile time, with typed Rust splices (`#lhs`)
  type-checked against the spelled IR types. Reuses `llvmkit-asmparser`'s
  lexer at proc-macro time for tokenization fidelity. Design sketch: parse to
  the existing instruction payload shapes, emit `build_*` calls; unsupported
  constructs fall back to a clear compile error naming the LangRef construct.
- **Rustc-quality diagnostics** -- when runtime checks do fail (dyn paths,
  parsed IR, verifier), render labeled spans into the printed IR with
  expected/found notes and suggestion hints. Builds on `llvmkit-support`'s
  `Span`/`SourceMap` (already used by the parser) plus a renderer; verifier
  errors gain an optional pretty-print path that quotes the offending
  instruction line from AsmWriter output. Candidate crate: keep in
  `llvmkit-support` as a `diagnostics` module.

## Upstream IRBuilder coverage gaps (from the comparison audit)

Signatures below are verified against the extracted `llvmorg-22.1.4` tree
(`orig_cpp/llvm-project-llvmorg-22.1.4/llvm/include/llvm/IR/IRBuilder.h`).

- Convenience casts: `CreateZExtOrTrunc`, `CreateSExtOrTrunc`,
  `CreateIntCast`, `CreateFPCast`, `CreateBitOrPointerCast`
  (IRBuilder.h ~1951-2038).
- Memory intrinsics: `CreateMemCpy` / `CreateMemSet` / `CreateMemMove` (each
  with `uint64_t` + `Value*` size overloads, plus `*Inline` and
  element-atomic variants); lifetime intrinsics `CreateLifetimeStart/End` --
  NOTE: in LLVM 22 these take only a pointer (size argument removed, allocas
  only; verified against 22.1.4).
- `CreateGlobalString` (needs globals + builder hookup; upstream
  `CreateGlobalStringPtr` is deprecated in its favor), `CreateAssumption`
  (takes operand bundles), the min/max family (`CreateMinNum`/`CreateMaxNum`
  with `FMFSource`, plus
  `CreateMinimum`/`CreateMaximum`/`CreateMinimumNum`/`CreateMaximumNum`), the
  intrinsic helper family (`CreateIntrinsic` 3 overloads,
  `CreateUnaryIntrinsic`, `CreateBinaryIntrinsic` -- the latter returns
  `Value*` because it folds), `CreateStepVector`, `CreateAggregateRet`
  (explicitly deferred in AGENTS.md).
- FMF-variant family completion: `CreateSelectFMF`, `CreateFPTruncFMF`,
  `CreateFPExtFMF` analogs (llvmkit has binop/fcmp `_fmf` variants already);
  consider an `FMFSource`-style "inherit FMF from instruction" helper.
- Const-index GEP shortcuts (`CreateConstGEP1_32` etc.).
- Named `build_icmp_*` per-predicate wrappers already exist; audit found no
  gap there.
- Debug-loc threading and operand-bundle infrastructure (deferred with
  metadata work).
- RAII-style `InsertPointGuard` / `FastMathFlagGuard` analogs (Rust shape:
  scoped closure `with_insert_point(bb, |b| ...)` rather than Drop guards).

## Ergonomics backlog (from the core audit)

- `Display` for the ~25 typed instruction handles (`LoadInst`, `CallInst`, …).
  Cycle C gave `Display` to every public *value* handle, which prints the
  operand form, but deliberately stopped at the instruction handles: their
  natural rendering is a full instruction line, not an operand, so they need
  an explicit decision (delegate to `InstructionView`'s `Display`, or print
  the operand form for consistency with the value handles) rather than a
  mechanical sweep. Whichever is chosen should be stated in each impl's
  rustdoc, as the value handles now do.
- `build_atomic_cmpxchg` / `build_atomicrmw` builder-pattern variants (mirror
  `CallBuilder`).
- Load/store variant explosion (base / `_with_align` / `_volatile` /
  `_volatile_with_align` / `_atomic` = 10+ methods per op) -- consolidate
  behind `LoadBuilder`/`StoreBuilder` chainables while keeping the flat
  forms.
- Per-flag convenience wrappers (`build_int_add_nsw` etc.) mirroring upstream
  `CreateNSWAdd`.
- Folder trait ergonomics for third-party folders (default method bodies
  landed in this session's hardening workstream; a
  `TargetFolder`/`InstSimplifyFolder` analog remains future work).

## Inspiration-derived candidates (web-researched)

- **"No-panic" positioning vs inkwell** (marketing + README bullets,
  near-zero code): inkwell's own docs and issue tracker document runtime
  panics on misused conversions (`into_float_value()` on an int panics --
  e.g. [wasmer#962](https://github.com/wasmerio/wasmer/issues/962)), panics
  on interior-NUL strings, and no multithreaded mode ([inkwell
  README](https://github.com/TheDan64/inkwell)). llvmkit's counterpart story
  is exact: typed handles make conversion misuse a compile error, there are
  no C strings anywhere, and every crate is `#![forbid(unsafe_code)]`. This
  session's README update (Task 20) turned this into a "why llvmkit vs
  inkwell" comparison section; a fuller marketing pass (blog post, crates.io
  description) remains future work.
- **E-graph optimization substrate** (L, future): an equality-saturation
  InstCombine/peephole analog built on
  [egg](https://github.com/egraphs-good/egg)/egglog -- Cranelift is already
  exploring e-graph-based optimization ([SIGPLAN
  blog](https://blog.sigplan.org/2021/04/06/equality-saturation-with-egg/)).
  llvmkit's typed constant-fold kernels + pass infrastructure give it a
  natural home as a `PatchBody`/`ReshapeCfg`-rung pass family. Would be a genuine "LLVM 2.0"
  differentiator: phase-ordering-free peepholes.
- **Alive2-style refinement checking** (L, future, visionary):
  [Alive2](https://github.com/AliveToolkit/alive2) does bounded translation
  validation of LLVM transforms via SMT (found 47 bugs in LLVM's own test
  suite). A llvmkit-native `refines(before, after)` harness --
  property-test-based initially (interpret both modules over random inputs
  for the modeled subset), SMT-backed later -- would make llvmkit the only
  IR library with built-in transform validation. Pairs with Doctrine D10 (no
  silent UB).
- Note: the full 5-lens inspiration sweep + synthesis workflow did not
  complete during planning; the three findings above come from direct
  main-session searches instead. If deeper inspiration mining is wanted,
  see the session's archived plan for the sweep's methodology and rerun it.

## Type-system follow-ups

- ~~**Block-argument edges are only half-guarded, and only on half the
  terminators**~~ (found 2026-07-27 re-reading
  `docs/design/phi-type-guarantees-design.md` against the tree; the design
  promised both halves and neither was noticed missing for 17 days, through the
  0.0.4 freeze) — **done (2026-07-27, `feature-34/polish-freeze`).** Both gaps
  were closed in one change, as the entry insisted they had to be: closing
  either alone would have left `br`/`cond_br` guarded while `switch`/`invoke`
  silently were not.

  1. **A plain branch into a parameterised block is now rejected.**
     `IRBuilder::build_br` / `build_cond_br` / `build_switch(_dyn)` (default
     edge) / `SwitchInst::add_case` / every `build_invoke*` (both edges) /
     `build_callbr*` (default and indirect edges) /
     `IndirectBrInst::add_destination` all route their successors through one
     guard, `basic_block.rs::require_no_block_parameters`, which reports the
     existing `IrError::PhiArgArityMismatch` — the same error a wrong argument
     *count* already got from `build_br_with_args`, so one mistake reads the
     same wherever it is caught. The check runs before the terminator is
     emitted, so a rejected edge leaves no half-formed instruction.

     **The early-out is not the one this entry proposed, and the difference is
     load-bearing.** "Is the target's first instruction a phi?" would have
     broken the two authoring paths that legitimately branch into a block whose
     head-phis are not block *parameters*: the `.ll` parser (a back-edge to an
     already-parsed loop header) and `SsaBuilder` (a back-edge to an unsealed
     header whose reads have minted operandless phis, completed later at
     `seal_block`). Both seed those phis through their own checked paths, and
     neither can spell an argument list. So a block instead records the
     parameter count it was *created* with — a `Cell<usize>` on
     `BasicBlockData`, set only by `append_block_with_params`,
     `append_block_with_named_params`, and `append_block_typed` — and the guard
     early-outs on that single read. It is cheaper than the proposed scan-gate
     (no instruction-list touch at all on the hot path) and it is the more
     honest predicate: *parameterised* is a property of how the block was
     authored, not of what its first instruction happens to be. The scan
     survives as the shared `block_parameter_phis`, which both `add_block_args`
     and the guard's error message read, so "how many parameters" has one
     definition. `plain_branch_into_auto_ssa_phi_block_still_builds`
     (`tests/block_args_terminators.rs`) pins the distinction.

  2. **`switch` and `invoke` have argument-carrying forms.**
     `build_switch_with_args` / `build_switch_dyn_with_args` take the default
     edge as a `(target, args)` pair plus a `(case_value, target, args)` triple
     per case — the whole case list at the call, so the returned `SwitchInst`
     is already `TermClosed` and no later `add_case` can bolt on an unseeded
     edge. `build_invoke_with_args` / `build_invoke_dyn_with_args` take a
     `(destination, args)` pair for each of the two mandatory edges. All four
     bundle each edge with its arguments into one parameter (the case list
     forces that shape, and it keeps `invoke`'s call arguments and result name
     out of an eight-parameter signature), and all four validate arity and
     argument types up front, per edge, exactly as `build_cond_br_with_args`
     does — sharing its documented non-atomicity across edges.

  **Residual, deliberately reject-only:** `indirectbr`, `callbr`, and the
  *indirect-callee* and *inline-asm-callee* `invoke` shapes have no
  argument-carrying twin — an `indirectbr`/`callbr` indirect edge is selected at
  run time, so there is nothing to hang a per-edge argument list on, and the two
  exotic invoke callees would need their own signature explosion for no known
  consumer. A parameterised destination is rejected there rather than silently
  under-seeded; authoring a phi in such a block goes through `SsaBuilder` or
  `FnReshape::insert_phi`, on a block created with plain `append_basic_block`.
  For `indirectbr` this is precisely the restriction the design asked for.

- ~~**Six `#[cfg_attr(not(test), allow(dead_code))]` violate the `#[allow]` ban**~~
  — **done (2026-07-27, at the 0.0.4 freeze).** AGENTS.md bans `#[allow(...)]`
  unconditionally — "not anything else" — and a `cfg_attr` wrapper does not
  exempt it. All six sat on the crate-internal raw-phi authoring surface
  (`PhiInst`/`FpPhiInst`/`PointerPhiInst::add_incoming` in `instructions.rs`,
  and `IRBuilder::build_int_phi`/`build_fp_phi`/`build_pointer_phi`), which went
  dead in non-test builds when block arguments took over as the public
  phi-authoring surface.

  Fixed the way the law prescribes — drop the dead code — by marking all six
  `#[cfg(test)]`, since their only callers were ever `src/phi_raw_tests/`. Two
  imports (`IntoFloatValue`, `IntoPointerValue`) became test-only with them and
  are gated the same way rather than suppressed.

  This re-pointed `tests/compile_fail/raw_phi_builder_is_unnameable.rs`: it used
  to prove the builders are *private* (`E0624`) and now proves they *do not
  exist* in a dependent crate's build (`E0599`). That is the stronger claim — a
  private method still exists and a later `pub` slip would expose it, whereas a
  method compiled out cannot be reached at all. Fixture doc comment rewritten
  and `.stderr` regenerated on 1.96.0.

- ~~**Metadata is the one currency 2.0 did not tag**~~ — **done (2026-07-27, at
  the 0.0.4 freeze).** Found during the cycle E freeze sweep and pre-existing
  rather than a 2.0 regression: 2.0 tagged the *value* currency and left this
  one behind. `MetadataSlot` was a bare `usize` arena index and the `ValueSlot`
  inside `DebugMetadataOperand::Value` was likewise bare, so **neither half of
  D7 reached metadata** — no `B` for two modules' handles to differ in, and no
  tag for the arena boundary to check. An *in-range* slot minted in module A and
  attached in module B resolved against **B's** arena in
  `asm_writer::fmt_debug_metadata_operand` / `fmt_metadata_operand` and printed
  the wrong node, silently. Cycle E had bounded the *reachability* of that
  mistake (every attach point demands the target module's `Unverified` token,
  and out-of-range slots became `IrError::UnknownMetadataSlot`) without closing
  the hole.

  Closed by mirroring cycle A's value split exactly:

  - `MetadataSlot` stays the bare arena index and became **crate-internal**,
    alongside `MetadataStore`. It is reachable only from the storage side.
  - **`MetadataId<B: ModuleBrand>`** is the public currency —
    `{ tag: ModuleId, slot: MetadataSlot }`, `Copy + Send + 'static`, brand
    phantom `PhantomData<fn(B) -> B>`. `MetadataRef` (a `pub` newtype with a
    `pub` field — a forgery hole of its own) is gone; `MetadataId` replaces it
    everywhere.
  - Every vocabulary type that carries a metadata reference gained the `B`:
    `MetadataKind`, `SpecializedMetadataNode`, `MetadataField`,
    `MetadataFieldValue`, `DebugRecord`, `DebugVariableRecord`,
    `DebugMetadataOperand` (whose `Value` arm now carries `ValueId<B>`), and
    `MetadataAttachmentSet`. `ModuleCore` is brand-free and cannot store a
    generic, so the arena holds those same types at a crate-private
    `StoredBrand`; the two forms meet at exactly two crate-internal
    conversions — `into_stored`, which performs the tag check, and
    `from_stored`, a pure retag of ids the arena already owns.
  - The check lands at **one** choke point. `MetadataId::slot` exists only on
    `MetadataId<StoredBrand>`, so the only route from a caller's id to an arena
    index is `MetadataId::into_stored` / `ModuleCore::metadata_slot_of`, which
    compares the `ModuleId` first. Forgetting the check one level up is not
    expressible.
  - A foreign id is **`IrError::ForeignMetadataId`** (new; the metadata twin of
    `ForeignValueId`) on every entry point that accepts one, which made
    `metadata_tuple` / `metadata_tuple_with_distinct` / `metadata_specialized` /
    `metadata_node` / `metadata_as_value`, every `set_metadata` setter
    (instruction, function, and the three globals), and `push_debug_record`
    fallible. `metadata_constant` joined them for the same reason on the value
    side. `UnknownMetadataSlot` keeps the
    out-of-range case, now reachable only for a *native* id.

  The `.ll` parser holds `MetadataId<B>` in its `!N` bookkeeping and needed no
  raw-slot escape hatch: every id it hands back was minted by the module it is
  populating. Printed IR is byte-identical — the byte-locked example suites and
  the parser round-trip corpus are unchanged. Locked by
  `tests/module_ownership.rs::a_metadata_id_from_another_module_is_refused_everywhere`
  (runtime tag, two `DynBrand` modules with identically-shaped arenas) and
  `tests/compile_fail/cross_module_metadata_attachment.rs` (two named brands).

- **Const-generic `VectorType<E, Len<N>>` / `ArrayType<E, ArrLen<N>>` — shipped**
  (`feature-17/const-generic-vec-array`, S1–S6). `VectorType`/`VectorValue` and
  `ArrayType`/`ArrayValue` now carry a scalar **element** marker (the scalar
  itself — `i64`, `f64`, … via `VecElem`/`StaticVecElem` in `element.rs`) and a
  **length** marker (`Len<const N: u32>`/`LenDyn` in `vec_len.rs`;
  `ArrLen<const N: u64>`/`ArrLenDyn` in `array_len.rs`). The bare
  `VectorValue<'ctx>`/`ArrayValue<'ctx>` stay the all-`Dyn` (erased) form —
  parsed `.ll`, scalable vectors, and runtime lengths land there and narrow via
  `TryFrom` (`OperandWidthMismatch` for lane count, `IrError::ArrayLengthMismatch`
  for arrays, `TypeMismatch` for element). Constructors
  `Module::vector_type_n::<E, const N>()` / `array_type_n::<E, const N>()`. Typed
  ops make an element/length mismatch a **compile error**:
  `build_vec_int_{add,sub,mul,xor,and,or,shl,lshr,ashr}` (two
  `VectorValue<E, Len<N>>` with the same `E`,`N` ⇒ equal element+length for
  free), `build_vec_extract`/`build_vec_insert`/`build_vec_splat`, and array
  `build_arr_extract`/`build_arr_insert` (plus a typed-array `build_alloca`); the
  verifier's vector/array checks are unchanged (defense in depth). The old
  unwired `VectorElement`/`SizedElement`/`VectorDyn`/`ArrayDyn` markers were
  replaced by `VecElem`/`ElemDyn`. Residual, deliberately still erased / `Dyn`:
  - **Length-relating ops** — shufflevector output length, concat (`N1+N2`),
    compile-time index-in-bounds (`I<N`), cross-`Len` widen/narrow — **blocked on
    `generic_const_exprs` (unstable)**, the same wall as the integer `WiderThan`
    relations below.
  - **Scalable vectors** — always `Dyn` (**scoped out** this cycle).
  - **Pointer-element vectors** — **scoped out**, blocked on address-space
    markers (see the address-space-typed-pointers bullet below).
  - **Composite-element arrays** (`[N x {..}]` / `[N x [..]]` / `[N x <..>]`) — a
    scalar element marker can't name a composite element (**scoped out**).
  - **Float / div / rem vector binops and vector `icmp`/`fcmp`** — **scoped out**;
    no existing erased `_dyn` lowering to reuse.
  - `build_vec_splat` can't infer its element from the scalar (a Rust
    associated-type-projection limitation), so its callers annotate / turbofish
    the result.
- **A proof token that *carries* the validated `TypeId`** (residual after the
  unforgeable-markers cycle). The crate has five capability tokens -- `WrapWitness`
  (`element.rs`), `ValidatedFunctionParams` / `ValidatedCallResult`
  (`function_signature.rs`), `SelectNarrow` (`ir_builder.rs`), `ValidatedStructValue`
  (`struct_schema.rs`) -- each defending the *external* boundary and each a *unit*
  marker that proves "a check happened", not *which* type. The unforgeable-markers
  cycle made the builder's **int / float / pointer append surface** structural instead:
  a marker is attached to a freshly-appended instruction only through the typed-append
  constructor family (`append_int_like` / `_at` / `_load`, `append_fp_*`, `append_ptr`
  / `append_ptr_load`), each of which appends AT a typed handle so the marker matches
  the runtime type *by construction* — those ~40 sites no longer carry an implicit proof.
  What remains implicit is the smaller residual the family does not cover: the `CallInst`
  / `PhiInst` result accessors in `instructions.rs`, the arena / parameter lifts in
  `ssa_builder.rs` (`use_*_var`) and `function_signature.rs`, the vector / array append
  wraps (no `append_vec` / `append_arr` constructor yet), and the `IntoIntValue` /
  `IntoFloatValue` const-lifts in `int_width.rs` / `float_kind.rs`. A witness carrying the
  validated `TypeId` would let those *state* their proof instead of implying it. Note the
  confinement of `from_value_unchecked` is **audited, not compiler-enforced**: it stays
  `pub(crate)` because a hard seal is impossible (`value` and `ir_builder` are sibling
  modules and the constructors need `ir_builder`-private helpers), so the builder's fold
  re-checks remain the runtime backstop.
- `Width<M>`/`Width<N>` `WiderThan` relations blocked on stable
  const-generics (documented at `int_width.rs` ~105-116); revisit when
  `generic_const_exprs` stabilizes.
- Aggregate variable categories for auto-SSA (currently ships int/float/pointer
  only).
- Address-space-typed pointers (`PointerValue` currently erases address
  space; audit item from infra report).
- **Infallible statically-sized aggregate constants.**
  `ArrayType<E, ArrLen<N>>::const_array([C; N])` /
  `VectorType<E, Len<N>>::const_vector([C; N])` could drop the `IrResult`
  entirely — the length is known at compile time and the elements would be
  materialized at the type-level element `E` — but this needs typed
  `ArrayType<E, ArrLen<N>>` / `VectorType<E, Len<N>>` **type constructors**
  first: today every `const_array`/`const_vector` caller builds an *erased*
  `ArrayType<ElemDyn, ArrLenDyn>` / `VectorType<ElemDyn, LenDyn>` via
  `m.array_type(...)` / `m.vector_type(...)`, so an infallible static
  constructor would be dead code (no statically-typed receiver exists to call
  it on). The fallible `const_array`/`const_struct`/`const_vector` now accept
  `impl IntoConstantValue` elements (literals work), but stay `IrResult` — the
  element-vs-container type check still guards the erased receivers. Bundle the
  infallible variants with a typed aggregate-**type**-constructor slice; the
  same applies to a `StructSchema`-keyed tuple-struct constant.

## Session follow-ups

Items this session's own workstreams punted, beyond the plan's original
future-work list above. Each cites the source file/design decision that
deferred it.

- **Typed `fold_gep`/`fold_select` hooks** -- blocked on address-space-carrying
  pointer markers; `PointerValue` doesn't pin the address space and vector
  element typing is deferred to T4, so `fold_gep_dyn`/`fold_select_dyn` stay
  erased + runtime-checked (documented in `ir_builder/folder.rs` trait
  rustdoc).
- **`[F; N]` `IrField` arrays** -- fixed-size array fields in `#[derive(IrStruct)]`
  schemas; would let derived structs model `[i32; 4]`-shaped LLVM array
  members directly instead of requiring a hand-written wrapper.
- **Vector-of-pointer GEP bases** -- `build_gep`/`build_field_gep` currently
  assume a scalar pointer base; vectorized GEP (`<N x ptr>` base, per-lane
  offsets) is unmodeled.
- **Derive-generated field-index consts** -- `build_field_gep::<S, I>` takes
  the field index as a bare `const I: u32`; the derive macro could emit named
  constants (e.g. `Point::X_INDEX`) so call sites read `build_field_gep::<Point,
  { Point::X_INDEX }>` instead of a magic number.
- **`TypedInvokeInst<Ret>` schema wrapper** -- `build_invoke` returns
  `InvokeInst<Ret::Marker>` today; a `TypedCallInst`-style wrapper carrying
  the full `Ret: FunctionReturn` schema (not just the derived marker) is
  mechanical follow-up work noted in the typed-calls design (Workstream 1)
  as "deferred (mechanical later, reuses `CallArgs` unchanged)".
  Same design note also defers typed `callbr`, typed intrinsic calls, and
  varargs invoke -- all mechanical extensions of the shipped `CallArgs`/
  `IntoCallArg` machinery.
- **Auto-SSA aggregate variables + invoke/EH terminators** -- `ssa_builder.rs`
  currently ships int/float/pointer variables and `br`/`cond_br`/`switch`/`ret`/
  `ret_void`/`unreachable` terminators only. Aggregate variable categories
  (per-field fan-out through `StructSchema`) and `invoke`/`callbr`/EH
  terminators are the documented future scope in the module's own doc comment.
- **`IrField::ir_type` accepting a module view -- done** (0.0.4 cycle C,
  `feature-32/owned-modules`). `IrField::ir_type`, `StructSchema::field_types` /
  `ir_type`, `FunctionReturn::ir_type`, `FunctionParam::ir_type` and
  `FunctionParamList::ir_types` now take `ModuleView<'ctx, B>` instead of
  `&Module<..., Unverified>`, so `build_field_gep` no longer wraps a temporary
  module token to call `S::ir_type(...)` — `Module::from_core` is gone with it.
  Type construction is preservation-neutral, which is why the view already
  carried the constructor surface.
- **`proptest` `undef_var` index randomization** -- the auto-SSA property
  test suite's undefined-variable-read fixture hardcodes `Some(0)` as the
  undefined variable index instead of drawing from `0..var_count`; a one-line
  improvement to widen coverage (noted during Task 19's review).
- **`accept_folded/narrow_folded` helper-family factoring -- done**
  (`feature-22/generic-narrowing`, the "no silent erasure" cycle). The four
  near-identical bodies were folded into a single compare-and-report core,
  `Type::require_match` (`type.rs`), which every fold- and variable-def seam
  now routes through -- so the same type drift reports the same error wherever
  it is caught. That unification was not the goal but a consequence: the seams
  had to be touched anyway to delete a marker-keyed short-circuit, and leaving
  four copies would have meant four places for the error shape to diverge again
  (it already had -- `narrow_folded_int` reported `TypeMismatch { Integer,
  Integer }` where the acceptor reported `OperandWidthMismatch`).

## Upstream-parity review follow-ups (2026-07-06)

A six-agent audit of the shipped overhaul against
`orig_cpp/llvm-project-llvmorg-22.1.4/` confirmed the builder semantics clean
and produced two fix waves: the first (fold_phi poison skip,
definitive-initializer gate, i128 sign-extension, SSA poison-arm RAUW + chase
cycle detection, any-order flag parsing, call-site fn_ty independence), and the
LLVM 22.1.4 parity-completion pass (DataLayout default alignment on
load/store/alloca, alloca array-size / `inalloca` / `swifterror` / DL alloca
address space, GEP index validation, indirect invoke, musttail ellipsis rules,
unordered-atomic-load DCE + trivially-dead InstSimplify erase, and the
`llvmkit-default<On>` recipe rename). The `build_is_null`/`build_pointer_cmp`
folder-bypass item was already fixed on dev (`b06413e`). The items below remain
deliberately deferred; each cites its upstream anchor.

- **Indirect `callbr`** -- `callbr void %fp(...)` is invalid IR upstream
  (`Verifier::visitCallBrInst` requires a direct callee for non-asm callbr:
  "Callbr: indirect function / invalid signature"), so llvmkit rejects it at
  parse, which reaches the same verdict. A stricter port would accept it at
  parse and reject in the verifier. (Indirect *invoke* is now supported -- it
  is valid IR.)
- **DCE removable calls / allocs** -- llvmkit still keeps `willReturn`+readnone
  calls, removable allocation-function calls, `free(null)`, and lifetime-only
  allocas that upstream `wouldInstructionBeTriviallyDead`
  (`lib/Transforms/Utils/Local.cpp`) deletes. Porting these needs faithful
  allocation-function / attribute modeling to avoid over-removal (a miscompile
  if wrong), so the current DCE stays conservative-but-safe. `Value::has_uses`
  also counts debug-record uses upstream ignores (upstream salvages debug info
  instead). (Unordered atomic loads are now removed.)
- **InstSimplify unreachable-block skip** -- the pass still folds in
  unreachable blocks that upstream skips (`InstSimplifyPass.cpp:33-37`), a
  textual-only divergence in dead code; needs reachability (a dominator tree)
  threaded into the pass. No InstSimplify tests cover freeze folds or
  unreachable-block behavior yet (the latter blocked on this skip). (The
  erase-only-when-trivially-dead behavior is now matched.)
- **Deeper `swifterror` dataflow verification** -- the swifterror alloca
  support verifies the parse-level constraints (pointer type, non-array); the
  full `Verifier` use-site rules (swifterror values may only flow through
  specific call/load/store positions) are not yet enforced.
- **Plain add/sub/div/shift hook dispatch** -- `build_int_add`/`build_int_sub`
  consult the plain `fold_int_bin_op` hook where upstream `CreateAdd` funnels
  through `FoldNoWrapBinOp(.., false, false)` (and `CreateUDiv` et al. through
  `FoldExactBinOp(.., false)`). Identical results with the shipped folders;
  observable only by third-party folders that override just the
  no-wrap/exact hooks.
- **Vector-of-pointer GEP bases** -- `build_gep` / the parser assume a scalar
  pointer base; `<N x ptr>` vector GEP bases (`getGEPReturnType`'s vector arm)
  are unmodeled (documented earlier in this file). Consequence for the new GEP
  index validation: the struct-index-must-be-`i32` check (`StructType::indexValid`,
  upstream `isIntOrIntVectorTy(32)`) is enforced for scalar indices only; the
  `<N x i32>` vector-index case is unreachable here because a vector-index GEP
  requires a vector base, which is rejected earlier. Revisit the check when
  vector GEP bases land.

## Pass API — deferred

The `feature-4/pass-api-v2` branch shipped the capability-graded pass API
(rungs, contexts/mutators, `FunctionPass`/`ModulePass`, single-pass drivers,
static tuple pipelines, `Analyses` bundle, `Dyn` containers, and the
`#[function_pass]`/`#[module_pass]` sugar). What it deliberately scoped out:

- **Executable textual / string pipelines** -- `pass_pipeline.rs` parses
  opt-style pipeline strings into names and recipes
  (`parse_pass_pipeline_text`, `PassPipelineRecipe`, `PassPipelineTextName`),
  but there is no `NAME`->pass-constructor registry, so a parsed pipeline
  cannot yet be *run*. A registry mapping each pass's `NAME` to a boxed-pass
  constructor would let a textual pipeline drive the `Dyn` containers.
- **Per-function analyses in `ModRewrite::patch_functions` /
  `reshape_functions`** -- the module->function iterators (`pass_context.rs`)
  build each per-function mutator with empty results `()`, so a
  `FnPatch::analysis` call inside the loop has no members to select. A future
  revision threads a per-function `Requires` list (prefetched per yielded
  function) through them.
- **Instrumentation wiring** -- the `const NAME` / `const REQUIRED` pass
  members and `PassInstrumentationCallbacks` (`pass_instrumentation.rs`) exist,
  but the single-pass drivers and pipelines (`pass_manager.rs`) do not yet fire
  before/after-pass instrumentation callbacks or honor skip decisions. The
  `pass_names()` / `has_required_pass()` accessors on the `Dyn` containers are
  the surfaced hooks awaiting a consumer.
- **Loop and CGSCC rungs** -- the capability lattice (`pass_access.rs`) covers
  single-function-body and whole-module rungs only; loop-nest and
  call-graph-SCC pass rungs (upstream `LoopPassManager` / `CGSCCPassManager`)
  are unmodeled.
- **Typed `BlockCall` redirect twins -- REFUTED, not deferred.** A typed
  `redirect_*_call(BlockCall<Params>)` surface on the pass-side edit handles
  (`BrEdit`/`SwitchEdit`/`InvokeEdit`/`CallBrEdit`, `pass_context.rs`) would
  move the phi-seeding arity/type check from run time to compile time, the
  redirect analog of the shipped typed *creation* path
  (`build_br_call`/`build_cond_br_call`). It was planned for cycle D and cut
  after a four-source audit found it out-types every relevant consumer:
  - **LLVM** has no atomic redirect at all -- the idiom is a manual two-step
    (`replaceSuccessorWith` / `SwitchInst::setSuccessor` then
    `BasicBlock::removePredecessor` to fix phis by hand). llvmkit's *erased*
    `redirect_*(new_to, phi_values)` is already stronger (atomic, validated).
  - **MLIR** -- the one upstream built around block arguments -- passes
    successor operands *dynamically* (`BranchOpInterface::getSuccessorOperands`
    returns a mutable `SuccessorOperands` range checked by the runtime
    `verifyBranchSuccessorOperands`), not by a static per-block signature.
  - **Mergen** (the C++ lifter this project's `bin_lift` consumer reimplements
    and extends) redirects/rebuilds edges only on *runtime-recovered* CFGs:
    `CustomPasses.hpp` rebuilds a `switch` with a runtime case count
    (`CreateSwitch(op, default, newNumCases)` + `addCase` over discovered
    constants) and seeds phis by looping `addIncoming(v, cb)` over runtime
    predecessor vectors, returning `PreservedAnalyses::none()` -- the erased
    `ReshapeCfg` floor. A static `BasicBlockLabel<Params>` schema is unknowable
    in principle there: case counts, targets, and phi incomings are *recovered*,
    not *authored*.
  - **`bin_lift`'s `lift-core` `IrBuilder`** is creation-only (`new_block` /
    `br` / `cond_br` / `switch`), with no redirect/split/phi/block-param surface.

  Because pass-side labels arrive erased-by-origin (from
  `FunctionView::basic_blocks()`), a typed twin would also force a narrowing
  round-trip (`try_into_typed::<Params>`) that the erased redirect avoids. Typed
  `BlockCall` earns its keep at block *creation*, where the typed label is in
  hand; it has no consumer at pass-side redirect. Revisit only if a pass appears
  that authors fresh, statically-shaped merge blocks *and* redirects existing
  edges into them -- a shape none of the four surveyed systems exhibits.
- **First-class `ModRewrite` runtime-symbol/global/ctor triple** -- the
  `RewriteModule` mutator (`ModRewrite`, `pass_context.rs` ~1247) exposes only the raw
  `module_mut()` token today; a sanitizer reaches the
  function/global/constructor "triple" through it by hand. The author sugar for
  that pattern -- `declare_runtime_fn` / `append_ctor` / `add_global` helpers
  plus the `llvm.global_ctors` machinery -- is deferred until an in-tree
  consumer needs it.
- ~~**`Module::scratch_unverified` footgun**~~ -- **done (2026-07-26, cycle
  cycle C1)**. `scratch_unverified` doesn't exist anymore. `feat(cycle C1): Module
  owns its ModuleCore` replaced it with `Module<B, Unverified>::assume_verified`
  (`module.rs` ~4090, still `pub(crate)`), called from the read-only `Dyn`
  pipelines' `run` methods (`DynReadOnlyFunctionPipeline::run` /
  `DynReadOnlyModulePipeline::run`, `pass_manager.rs` ~1819-1825 and
  ~1968-1976): `module.unverify()` hands out the token, every queued pass is
  `Inspect` so it projects to `()` and never reaches a mutator, and
  `unverified.assume_verified()` re-stamps that same token `Verified` with no
  re-verification. The original footgun doesn't apply anymore: a `Module` now
  *owns* its core by move rather than pointing at shared storage, so there is
  no way to mint a second live token over the same data. `assume_verified`
  round-trips the one token the pipeline already holds -- it doesn't conjure a
  throwaway one, so the caller-marker gap the old bullet worried about has
  nothing left to guard against.
- ~~**Compile-fail `.stderr` canonical-rustc bless**~~ -- **done (2026-07-26,
  cycle cycle D/E)**. There were never two "environmental" drifts: gated on the
  pinned CI toolchain (`cargo +1.96.0`), `folder_typed_wrong_width` and
  `extract_value_empty_indices` both pass, and the whole suite's baseline is
  **0 failures of 83 registered fixtures**. The mismatch only ever appeared
  when the suite was run on a *newer* rustc than the pin. Every `.stderr` in
  the tree is blessed on 1.96.0; re-bless there and nowhere else.

## Package 4 (analysis preservation) — deferred

Framework-witnessed analysis preservation shipped across `feature-8`
(Phase 1) and `feature-9` (the remainder): the `CfgUpdate` recording vocabulary
(`cfg_update.rs`), the `CfgIncremental` hook (`RepairOutcome` +
`apply_updates`/`recompute`), the reshape mutator's witnessed edit log, the
*unrepresentable* mid-reshape stale CFG-analysis read
(`FnReshape::analysis_repaired`, no `Deref`, compile-fail fixture),
`Requires`-without-`Default` (`PrefetchableAnalysis`), and the `done()`-flush
witnessing loop that keeps a reshape pass's dominator tree
(`DominatorTree::apply_updates` repairs correct-by-recompute → `Repaired`; the
driver marks preserved exactly what it watched repair). What remains deferred:

- **Sub-linear incremental dominator repair (perf).** `DominatorTree::apply_updates`
  is *correct* but repairs by full recompute — it does not yet use the recorded
  edge insert/delete list to do sub-linear work. A genuine incremental update
  (LLVM SemiNCA-style, driven by `updates`) is the perf follow-up. When it lands,
  a `debug_assert` comparing the incrementally-repaired tree to a from-scratch
  recompute (`repaired ≡ recomputed`) should guard every flush; the
  `dominator_tree_repairs_to_match_recompute` test is the seed of that property.
  Needs random-edit-sequence property tests (proptest). No behavior change vs.
  today when it lands (only speed), so low urgency without a large-function
  workload.
- **`PrefetchableModuleAnalysis` (module `Requires` without `Default`).** The
  function side dropped the `Default` bound via `PrefetchableAnalysis`; the
  module analysis-list macros still bound `+ Default`. There are no concrete
  module analyses yet, so a mirror trait would be untestable dead machinery —
  introduce it (same shape) with the first non-`Default` module analysis.
- **Value-analysis update vocabulary.** `CfgUpdate` is CFG-shaped only.
  Instruction-level events for value analyses (KnownBits/DemandedBits) are a
  possible extension, not designed here -- every mutating rung's floor already
  evicts them.
- **`ModRewrite::reshape_functions` reshape flush.** The module→function
  iterator yields `FnReshape` mutators whose `done()` (and thus `CfgUpdate` log)
  never reaches the driver, so those reshapes do not run the witnessed flush.
  This is sound today: the enclosing `RewriteModule` floor is `none()`, which
  evicts every CFG analysis anyway. Reclaiming the lost *precision* means
  raising that floor above `none()`, which in turn means witnessing the
  module-structural mutation `ModRewrite::module_mut` deliberately hands out
  unwitnessed -- so this waits on a decision about `module_mut`, not on
  plumbing alone. Wire the flush through the iterators if per-function analyses
  are ever threaded into them.
- **New `.stderr` under the canonical-rustc bless caveat.**
  `reshape_stale_cfg_analysis_across_edit` is blessed on the local rustc like
  the pass-API fixtures above; its `E0502` borrow-error wording is stable
  across toolchains, but it joins the set that should be re-blessed on the
  reference rustc.

## Phi authoring — shipped

The block-argument authoring surface (`append_block_with_params`,
`append_block_with_named_params`, `build_*_with_args`), dominance-witnessed
`FnReshape::insert_phi`, the "break" that made the raw phi builders (the six
`build_*_phi`, the open-phi `add_incoming`/`finish`) internal — block arguments
are now the *only* public phi-authoring surface — the **typed terminator edit
surface** (`FnReshape::edit_terminator` and the `edit_switch`/`edit_cond_br`/
`edit_br`/`edit_invoke`/`edit_callbr` narrows → `BrEdit`/`CondBrEdit`/
`SwitchEdit`/`InvokeEdit`/`CallBrEdit`) that *replaced* the dynamic
`remove_edge`/`redirect_edge` edge ops (`BranchInstData.kind` became a `RefCell`,
so a branch successor is retargeted and a `cond_br` collapses to a `br`,
deregistering the dead condition — now through role-named `redirect_*`/`remove_*`
whose very method set encodes the legal edits, so a structurally-invalid edge
edit is a *compile* error rather than a runtime rejection), the verifier
phi-result-type rule (`VerifierRule::PhiInvalidResultType`, defense in depth —
`check_phi` rejects a phi whose result is not a first-class data type, matching
the parser), and the zero-incoming-phi verifier backstop
(`VerifierRule::PhiEmptyInReachableBlock` — `check_phi` now rejects a phi that
carries no incomings in a block reachable from entry, gated on
`DominatorTree::is_reachable_from_entry`; such a phi is un-round-trippable
because `LLParser::parsePHI` rejects a bracket-less `%p = phi i32`, and the
shared `check_phi_incoming` count guard would otherwise miss it on the `0 == 0`
gap LLVM's `visitPHINode` shares) have all shipped.

The former follow-up here — **edge ops on `invoke`/`callbr`** — has also
shipped: the `invoke` (`normal_dest`/`unwind_dest`) and `callbr`
(`default_dest`/`indirect_dests`) successor `Cell`s are editable through the
typed edit surface — `edit_invoke` → `redirect_normal`/`redirect_unwind`,
`edit_callbr` → `redirect_default`/`redirect_indirect` — retargeting a successor
`Cell` in place exactly as the `br`/`cond_br` arms do. *Removal* is structurally
N/A for them: both `invoke` edges and the `callbr` default are mandatory and the
indirect count is fixed, and that absence is a compile-time guarantee (the
`InvokeEdit`/`CallBrEdit` handles carry no `remove_*`), not a gap.

**Typed block parameters** have also shipped (`feature-20/typed-block-params`) —
the block analog of the const-generic vector/array retrofit. A `BlockParams`
sealed marker with the erased `BlockParamsDyn` default sits as the last,
defaulted `Params` type parameter on `BasicBlockLabel`/`BasicBlock`, so all
erased authoring is unchanged; `IRBuilder::append_block_typed::<Params>` appends
a `Params`-stamped block with typed head-phi parameter handles; and the
`BlockCall<'ctx, R, B, Params>` edge (`head.call(args)` consumed by
`build_br_call`/`build_cond_br_call`) makes a wrong-arity or wrong-typed
block-argument a *compile* error. The erased surface
(`append_block_with_params`, `build_br_with_args`, `build_cond_br_with_args`) is
untouched. Two follow-ups remain deferred:

- **Edit-surface `BlockCall` integration.** The reshape edit surface's typed
  `redirect_*` phi-seeds stay erased (`&[Value]`): passes operate on `Dyn`
  block labels (`BasicBlockLabel<R, B, BlockParamsDyn>`), so a typed `BlockCall`
  built from a `Params`-stamped label is rarely usable at a redirect site — the
  pass would first have to recover (or carry) a typed label. Until a pass surface
  threads typed labels through, the typed `BlockCall` edge is a construction-time
  (`IRBuilder`) convenience only, and the edit surface keeps taking erased
  per-parameter value slices.
- **Typed params beyond arity 12.** `BlockParams` has a `Debug` supertrait and
  the standard library stops deriving `Debug` on tuples past arity 12, so a
  `>12`-arity typed parameter tuple cannot satisfy `BlockParams` even though it
  is a valid `FunctionParamList`; a block with more than twelve typed parameters
  must fall back to the erased `BlockParamsDyn` form. Lifting the ceiling needs a
  `Debug` path for larger tuples (drop the supertrait, or supply a manual `Debug`
  for a fixed-shape wrapper) — the same std-tuple `Debug` wall that caps typed
  function parameters.

**Typed terminator operands** have also shipped
(`feature-21/typed-terminator-operands`) — the program's move from a
terminator's *edges* to its *operands*. The `switch` condition/case integer
width is now a last, defaulted `W: IntWidth = IntDyn` parameter on
`SwitchInst<'ctx, P, B, W>`; `IRBuilder::build_switch::<W>` pins `W` and
its `add_case` carries an `IntoIntValue<'ctx, W, B>` bound, so a wrong-width
case value is a **compile error** (the erased `build_switch_dyn` keeps the runtime
`TypeMismatch` check). And `build_indirectbr`'s address bound tightened from
`IsValue` to `IntoPointerValue`, so a typed non-pointer jump address is a
**compile error** (the pointer-ness check moves from `verify()` to build time;
erased `Value` addresses are unchanged). Parser / SSA-builder paths and the
whole erased authoring surface are untouched.

With the edit surface, typed block parameters, and now operand typing all
shipped, the **"branching bugs impossible at the type level"** program's typed
surfaces are largely complete. What remains is deliberately out of scope rather
than pending:

- **Universal per-function branding (`build_body`)** — designed in full, then
  **deferred out of the type-safety
  program** after re-analysis against the actual consumers. The evidence points
  one way: (1) regular authoring gains nothing — the default is `FnErased`, so
  existing `position_at_end` sites stay byte-identical and untouched, leaving the
  brand pure opt-in ceremony for them; (2) the primary consumer, the `bin_lift`
  lifter, stores SSA registers as **function arguments** (for concolic
  constant-visibility) and **rebuilds CFGs from runtime-recovered structure**, so
  a compile-time, un-nameable per-function `'fid` fights its model rather than
  helping it — its edges are recovered, not statically authored.

  **The locked design, recorded here because its spec does not ship.** The
  original write-up lives under `docs/superpowers/`, which is gitignored, so it
  is unreachable for anyone reading the repository; these are the decisions
  worth keeping. `FunctionValue` gains a defaulted brand parameter
  `Fb: FnBrand = FnErased`, so every existing call site is unchanged. Opting in
  goes through `func.build_body(|fb| …)`, which mints a generative
  `FnScoped<'fid>` for the closure body; `build_br` (and the other
  branch builders) require the target label's `Fb` to match the builder's, so a
  label minted in one function's body is not the right *type* to branch to from
  another's. That is what makes a cross-function branch — LLVM's "Referring to a
  basic block in another function!" — a compile error instead of a verifier
  finding.

  Two things changed under it since: 0.0.4 deleted the closure-scoped
  module constructor, so `build_body` would reintroduce the one shape the
  redesign removed; and block targets are now storable `BlockId<R, B, Params>`
  ids rather than borrowed labels, so a per-function *lifetime* has nothing to
  attach to. A 2.0-shaped revival would need a per-function marker on `BlockId`
  itself. Until then the rule stays a `Module::verify()` check — see the "br
  target is not a basic block of the parent function" family in `verifier.rs`.
  Revisit only as its own opt-in cycle if a concrete authoring need appears.
- **Whole-graph verifier territory** — phi-incoming completeness against the
  final predecessor set for builder-constructed IR, and dominance, are permanent
  residents of `Module::verify()` (defense in depth). These are whole-graph facts
  that cannot be a local construction- or parse-time guarantee, so they stay the
  verifier's job by design, not a gap to close.

## Constant-folding parity — deferred / known divergences (2026-07-23)

The `feature-29/constfold-parity` cycle fixed the divergences a three-agent
audit found against vendored `llvmorg-22.1.4` (see CHANGELOG "Constant-folding
parity"). A whole-branch review confirmed no mis-folds and no over-folds; the
items below are the deferred / known-remaining points.

- ~~**Constants are not uniqued like upstream `Constant*`.**~~ **Closed
  2026-08-01.** `GlobalValueRef`, `GepOffset`, `SymbolDelta`, and
  `SymbolDeltaPlus` now intern on their structural fingerprint like every other
  constant kind, so identity comparison is structural everywhere and the
  `base_identity` workaround is retired. The under-folding arms this entry
  listed — `fold_phi`, select's `true_value == false_value`,
  `constant_splat_value`, and the pointer-base comparison — all read plain `==`
  now, matching upstream. Forward `blockaddress` placeholders stay un-uniqued
  by design (each is a distinct pending reference); see
  `tests/constant_uniquing.rs` for the law and its exceptions.

- **`ptrtoint`/`ptrtoaddr` mid-width on `pointer_size != index_size` (CHERI-like)
  layouts — a deliberate, reasoned divergence (needs a decision).** In the
  `isEliminableCastPair` case-11 *declined* sub-case (`MidSize < SrcSize &&
  MidSize < DstSize`), llvmkit's `fold_ptr_to_int_pair` always two-steps through
  the case-11 mid (`ptrtoint`→pointer size, `ptrtoaddr`→index/address size),
  whereas upstream falls to its switch path (`ConstantFolding.cpp` ~1508:
  `PtrToInt`→address type, `PtrToAddr`→int-ptr type — the inverse). Example on
  `p:128:128:128:64`: `ptrtoaddr(inttoptr(i128 x)):i128` → llvmkit `x mod 2^64`
  (the semantically-correct address extraction), upstream `x`. llvmkit's value is
  arguably the *more correct* side; matching upstream would introduce a wrong
  mask to copy an upstream quirk, only on layouts x86/bin_lift never use.
  Deliberately NOT matched; recorded for an explicit decision if CHERI parity is
  ever in scope.

- **`SymbolicallyEvaluateGEP` sub-cases not ported (each only declines, never
  mis-folds):** `CastGEPIndices` vector-index width normalization; nested-GEP
  `in_range` preservation; the null/`inttoptr`-nonzero-base → `inttoptr` fold
  (needs `mustNotIntroduceIntToPtr` / `APInt::insertBits`); "infer inbounds for
  GEPs of globals" (needs a dereferenceable-bytes query). Each produces weaker /
  fewer folds than upstream, never a wrong one.

- **Proactive ApFloat bit-exactness audit (deferred to its own cycle).** No
  constant-folder fix in this cycle touched ApFloat arithmetic, and the folding
  audit found `ap_float` structurally faithful and test-backed. A full
  bit-for-bit verification across all seven float semantics (incl. PPC
  double-double, every rounding/denormal/NaN-payload path) against known IEEE /
  LLVM `APFloat` values is a large, standalone effort worth its own cycle.
