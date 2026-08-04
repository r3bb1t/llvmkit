# Changelog

Notable, user-visible changes to `llvmkit`. The format follows
[Keep a Changelog](https://keepachangelog.com/); the project is pre-1.0, so
breaking changes are expected and are flagged inline. Until a tagged release is
cut, entries accumulate under **Unreleased**.

## [Unreleased]

### ValueTracking: the floating-point select arm and the context arm

#### Added

- `fp_predicate.rs` — `fcmp_implies_class` and `fcmp_to_class_test`, porting
  `llvm::FloatingPointPredicateUtils` and the generic implementation it
  instantiates (`GenericFloatingPointPredicateUtils`). Given `fcmp <pred> lhs,
  rhs`, they answer which classes `lhs` may belong to on each side of the
  branch. Each comes in three forms, differing in how the right-hand side is
  supplied: a value, an `ApFloat`, or a class mask. Upstream signals "nothing
  proved" with a null tested value and both masks wide open; that sentinel is an
  `Option` here, so a result in hand always carries a real answer.
- `adjust_known_fp_class_for_select_arm`, porting
  `llvm::adjustKnownFPClassForSelectArm`, and with it the `select` arm of
  `compute_known_fp_class`.
- The context arm of `compute_known_fp_class` — `computeKnownFPClassFromContext`
  and `computeKnownFPClassFromCond` — reading the same three sources as its
  known-bits sibling: an injected condition, the dominating branch conditions,
  and the `@llvm.assume` calls. The caches were already there from tranche 8.
- Fast-math flags on a `call` now parse and print (`call nsz float
  @llvm.sqrt.f32(...)`), matching `LLParser::parseCall` and
  `writeOptimizationInfo`, including upstream's rejection of flags on a call
  whose return type is not a floating-point scalar or vector. The storage
  already existed; nothing could reach it.

#### Fixed

- `compute_known_fp_class` took the dynamic denormal mode everywhere, on the
  premise that llvmkit models no `denormal-fp-math` attribute. It does —
  `FunctionValue::denormal_mode` — and upstream's own parse maps an *absent*
  attribute to `ieee`, not to dynamic. Results that depend on the mode
  (`sqrt`, `log`, `canonicalize`, the min/max family, and every comparison
  against zero) were weaker than upstream's; they now match.
- The `sqrt` arm read `nsz` directly. Upstream reads it through `Q.IIQ`, so a
  query built with `without_instruction_info` must not see it.
- `compute_known_fp_class` returned early for a value that is not an
  instruction, before consulting the context. Upstream's `if (!Op) return`
  comes *after*, so an argument constrained by an assumption or a dominating
  branch now gets that refinement.

The parity ledger moves `adjustKnownFPClassForSelectArm` to modeled — **89 of
101, 12 gaps**. `analyzeKnownFPClassFromSelect` stays a gap with a corrected
reason: it is declared in `ValueTracking.h` and defined nowhere in the LLVM
tree. The name occurs exactly once across `llvm/`, its own declaration, with no
definition and no caller, so there is no behaviour to port.

### ValueTracking: floating-point classification (tranche 7b and 7c)

#### Added

- `known_fp_class.rs` — `compute_known_fp_class` over the `fp_class` lattice,
  porting `llvm::computeKnownFPClass`, in the three overloads that differ in
  what they take: an interested-classes mask, everything, and a use site's fast
  math flags.
- The nine convenience predicates over it: `is_known_never_nan`,
  `is_known_never_infinity`, `is_known_never_infinity_or_nan`,
  `cannot_be_negative_zero`, `cannot_be_ordered_less_than_zero`,
  `compute_known_fp_sign_bit`, and the two `Use`-taking
  `can_ignore_sign_bit_of_zero` / `can_ignore_sign_bit_of_nan`.

**The dispatch is partial, and the module header names every arm that is
missing.** Ported: the constant and poison leaves, the fast-math-flag
refinement, `fneg`, `fpext`, `fptrunc`, `sitofp`, `uitofp`, and the
`fabs` / `copysign` / `sqrt` / `canonicalize` intrinsics together with the six
min/max, seven rounding, `exp` and `log` families. Not yet consulted: `select`
(it needs `adjustKnownFPClassForSelectArm`, still a recorded gap), the
assumption and dominating-branch arm, the arithmetic and vector arms, and the
remaining intrinsics. An unported arm leaves `fcAllFlags` — "could be anything"
— so it weakens an answer and never falsifies one.

`nofpclass` on a call return or parameter has no counterpart: llvmkit's
attribute model carries no such payload, so `getRetNoFPClass` and
`getNoFPClass` have nothing to read.

The parity ledger moves nine of the eleven tranche-7 rows, taking it to **88 of
101 modeled, 13 gaps**. The two that stay are the select-arm pair, whose reason
is rewritten: the lattice they needed now exists, only the dispatch arm is
absent.

### The floating-point classification lattice (tranche 7a)

#### Added

- `fp_class.rs` — `FpClassTest`, porting `llvm::FPClassTest`
  (`llvm/ADT/FloatingPointMode.h`): the ten-bit mask naming every class an IEEE
  value can fall into, split by sign, with upstream's named unions, the three
  transforms (`fneg`, `inverse_fabs`, `unknown_sign`), `APFloat::classify`, and
  the `Display` that ports `operator<<`.
- `KnownFpClass`, porting `llvm::KnownFPClass`
  (`llvm/Support/KnownFPClass.h`): that mask paired with a separately tracked
  sign bit, and every predicate over it —
  `isKnownNever*`, `cannotBeOrdered*`, `knownNot`, `fneg`, `fabs`, `copysign`,
  `propagateNaN`, `intersectWith`, `operator|=` and
  `isKnownNeverLogical{,Neg,Pos}Zero`.

This is the prerequisite for `computeKnownFPClass`, and stands to it as
`KnownBits` does to `computeKnownBits`. The *operations* on the lattice that
only `computeKnownFPClass` uses — `fmul`, `sqrt`, `log`, `exp`, `fpext`,
`roundToIntegral`, `canonicalize`, `minMaxLike`, `propagateDenormal`,
`propagateCanonicalizingSrc` — are deliberately not here yet: landing them
without their consumer would be a surface with no caller and no way to test it
against upstream. The module header records that.

`FpClassTest` is a `u32` newtype, not an `enum`. Upstream's is an `enum` only
because C++ has no other way to name a bitmask — `LLVM_DECLARE_ENUM_AS_BITMASK`
is upstream saying so — and `complement` accordingly complements within
`fcAllFlags` rather than within `u32`. `from_bits` is fallible where `bits` is
not: a mask read out of `@llvm.is.fpclass` is caller-supplied, and upstream's
verifier rejects an out-of-range one rather than truncating it.

### ValueTracking: assumptions and implied conditions (tranche 8)

#### Added

- `assumptions.rs` — the `@llvm.assume` slice. `find_values_affected_by_condition`,
  `is_valid_assume_for_context` and `will_not_free_between` port
  `ValueTracking.h`'s three entry points; `AssumptionCache` and
  `DomConditionCache` port `llvm/Analysis/AssumptionCache.h` and
  `llvm/Analysis/DomConditionCache.h`, which exist only to answer "which
  assumes / branches mention this value" and are built by calling the first.
  `Assumption` and `AssumptionSource` are the `ResultElem` pair, with
  upstream's `ExprResultIdx` sentinel spelled as a variant.
- `implied_conditions.rs` — `is_implied_condition` and
  `is_implied_by_dom_condition`, each in both the whole-condition and the
  decomposed spelling upstream declares, over the full static machinery:
  `isImpliedCondICmps`, `isImpliedCondFCmps`, `isImpliedCondAndOr`,
  `isImpliedCondCommonOperandWithCR`, `isImpliedCondOperands`,
  `isTruePredicate` and `getDomPredecessorCondition`.
- `compute_known_bits_from_context` and `adjust_known_bits_for_select_arm`,
  together with the `computeKnownBitsFromCond` / `computeKnownBitsFromICmpCond`
  / `computeKnownBitsFromCmp` chain they rest on. `compute_known_bits` now runs
  the context refinement where upstream does — after the operator walk — so
  every existing known-bits caller gets assumption- and branch-driven facts by
  attaching them to the query.
- `ValueTrackingQuery::{with_assumptions, with_dominating_conditions,
  with_condition_context}` and their accessors, porting `SimplifyQuery`'s `AC`,
  `DC` and `CC` fields. `CondContext` is the injected-condition vehicle.
- `PredicateWithSameSign` — upstream's `CmpPredicate` (`CmpPredicate.h`): a
  predicate plus the `samesign` flag its `icmp` carried, with `matching`,
  `preferred_signed_predicate`, `drop_same_sign` and
  `implied_by_matching_comparison` (`ICmpInst::isImpliedByMatchingCmp`).
  llvmkit's existing `CmpPredicate` stays the flag-less int-or-float union.
- `IntPredicate::signed_predicate`, porting `ICmpInst::getSignedPredicate`.

#### Fixed

- The integer min/max matcher behind `matchClamp` (tranche 4b) matched only the
  `select(icmp …)` spelling and accepted its operands in either order.
  Upstream's `MaxMin_match` also matches a direct `llvm.smin`/`smax`/`umin`/`umax`
  call, and `m_SMin` — as distinct from `m_c_SMin` — is not commutative. Both
  are corrected, and the select arm now binds operands in upstream's order (the
  *compare's*, with the predicate inverted rather than swapped when the true arm
  is the compare's right-hand side).

Two arms are narrower than upstream, each recorded at its site.
`computeKnownBitsFromContext`'s operand-bundle alignment refinement needs
`getKnowledgeFromBundle` (`llvm/Analysis/AssumeBundleQueries.h`), which is not
ported — `AssumptionCache` still records the bundle indices, so the arm can be
filled in without re-scanning. `isImpliedCondFCmps`'s constant-versus-constant
conclusion needs `ConstantFPRange`, which llvmkit does not model. Both omissions
only leave an answer weaker.

Where upstream builds a `ConstantInt::get(V->getType(), 0)` to feed the two
`m_NUWTrunc` arms of `isImpliedCondition`, llvmkit passes a bare `ApInt`
alongside the values instead: minting a constant would mean an analysis editing
the IR it was asked about. Operand equality follows LLVM's constant uniquing —
a literal equals a constant value holding the same bits — rather than raw value
identity.

### ValueTracking: select-pattern matching (tranche 4b)

#### Added

- `match_select_pattern`, `match_decomposed_select_pattern` and
  `can_convert_to_min_or_max_intrinsic`, completing the `SelectPatternResult`
  family whose vocabulary landed as tranche 4a. With them the whole static
  machinery: `matchClamp`, `matchMinMax`, `matchMinMaxOfMinMax`,
  `matchFastFloatClamp`, `getNotValue`, `lookThroughCast` and `isKnownNonNaN`.
- `SelectPatternMatch` — what upstream returns through its `Value *&LHS` /
  `Value *&RHS` / `Instruction::CastOps *CastOp` out-parameters. Upstream's own
  comment on those, "Assume success. If there's no match, callers should not
  use these anyway", is why the record sits behind an `Option`: a caller that
  did not match cannot read operands that were never meaningfully set.
- `FloatPredicate::{is_ordered, is_unordered, is_equality}` and
  `IntPredicate::is_equality`, porting the `CmpInst` predicates of the same
  names.

Three arms are narrower than upstream, each recorded at its site: fast-math
flags written on the `select` are not read (llvmkit's `select` carries no flag
word — flags on the `fcmp` are read, which is where they normally sit), and
`getNotValue` and `lookThroughCastConst` decline the cases that would need a
*new* constant to be minted, which is a module mutation. `can_convert_to_min_or_max_intrinsic`
answers `None` for the two floating-point flavours, the same gap already
recorded against `getInverseMinMaxIntrinsic`.

### ValueTracking: pointer and object analysis (tranche 5)

#### Added

- `pointer_analysis.rs` — a new module for the `getUnderlyingObject` /
  `findAllocaForValue` / `getConstantDataArrayInfo` slice of
  `ValueTracking.cpp`. Sixteen entry points: `get_underlying_object`,
  `get_underlying_object_aggressive`, `get_underlying_objects`,
  `get_underlying_objects_for_code_gen`, `pointer_base_with_constant_offset`,
  `find_alloca_for_value`, `only_used_by_lifetime_markers`,
  `only_used_by_lifetime_markers_or_droppable_instructions`,
  `argument_aliasing_to_returned_pointer`,
  `is_intrinsic_returning_pointer_aliasing_argument_without_capturing`,
  `get_constant_data_array_info`, `get_constant_string_info`,
  `get_string_length`, `is_bytewise_value`, `find_inserted_value`, and the
  `ConstantDataArraySlice` those last few read through.
- `BytewiseValue` — what `isBytewiseValue` found. Upstream returns a `Value *`
  because it can mint an `i8` constant; minting is a module mutation, so this
  returns the byte, the "any byte will do" answer, or the `i8` value upstream's
  first arm hands straight back.

Six of upstream's spellings become `Option` here, each because the `None`
carries information a `bool` plus an out-parameter does not:
`getUnderlyingObjectsForCodeGen` (which clears its out-parameter on failure),
`getConstantDataArrayInfo`, `getConstantStringInfo`, `GetStringLength` (whose
`0` means "cannot tell", not a length), `findAllocaForValue` (whose two failure
modes both leave the caller without an alloca), and
`ConstantDataArraySlice::move` (whose `assert(Delta < Length)` guards a caller
precondition).

`GetPointerBaseWithConstantOffset` returns the base and offset as a pair rather
than writing the offset through a reference, so the two cannot drift apart.

#### Changed

- `is_known_not_undef_or_poison` and its siblings now strip pointer casts
  before the allocated-object test, closing the second of the two arms that
  were recorded as deferred. A zero-offset `inbounds getelementptr` of an
  `alloca` is now recognised as the allocated object it points into — which is
  what upstream's comment on the strip says the strip is *for*, since that GEP
  would otherwise read as poison-capable. Only the `@llvm.assume` arm remains.

### ValueTracking: speculation safety and UB reachability (tranche 6)

#### Added

- `speculation.rs` — a new module for the `isSafeToSpeculativelyExecute` /
  `isGuaranteedToTransferExecutionToSuccessor` / `programUndefinedIfPoison`
  slice of `ValueTracking.cpp`. Thirteen entry points:
  `is_safe_to_speculatively_execute`,
  `is_safe_to_speculatively_execute_with_opcode`,
  `is_safe_to_speculatively_execute_with_variable_replaced`,
  `is_guaranteed_to_transfer_execution_to_successor` (plus the block and
  instruction-range forms `block_transfers_execution_to_successor` and
  `instructions_transfer_execution_to_successor`),
  `is_guaranteed_to_execute_for_every_iteration`,
  `may_have_non_def_use_dependency`, `must_trigger_ub`,
  `must_execute_ub_if_poison_on_path_to`, `program_undefined_if_poison`,
  `program_undefined_if_undef_or_poison`, `is_assume_like_intrinsic`,
  `is_not_cross_lane_operation` and `intrinsic_propagates_poison`.
- `SpeculationOptions` — upstream's two defaulted `bool` parameters
  (`UseVariableInfo`, `IgnoreUBImplyingAttrs`) as a named record, so a call
  site says which is which. `Default` reproduces upstream's defaults.
- `Opcode` and `OpcodeGroup` (`instr_types`), plus `InstructionView::opcode()`.
  Ports `Instruction::getOpcode` and the `HANDLE_*_INST` families of
  `Instruction.def` as one closed enum. Without it
  `isSafeToSpeculativelyExecuteWithOpcode`, whose whole purpose is to run the
  switch for an opcode other than the instruction's own, could not be ported at
  all. `UserOp1` / `UserOp2` are absent: `Instruction.def` reserves them for
  out-of-tree passes and llvmkit has no storage variant that could hold one.
- `MemoryEffects::does_not_access_memory`, `only_reads_memory` and
  `only_writes_memory`, porting the `MemoryEffectsBase` predicates of
  `ModRef.h`.
- `IntrinsicId::is_speculatable`, `will_return` and `no_free` — the TableGen
  properties LLVM materialises as function attributes on every intrinsic
  declaration.
- `MetadataAttachmentKind::NoUndef`, the `!noundef` attachment
  (`FixedMetadataKinds.def`'s `MD_noundef`). It previously fell through to
  `Custom("noundef")`, which is round-trip-correct but invisible to the
  analysis that reads it.

#### Changed

- `is_known_not_poison` / `is_known_not_undef` / `is_known_not_undef_or_poison`
  gained four of upstream's arms that were previously recorded as deferred:
  the `programUndefinedIfUndefOrPoison` walk, the dominating-branch-condition
  walk, the `shufflevector` splat arm, and `!noundef` / `!dereferenceable` /
  `!dereferenceable_or_null` on a `load`. The call-return arm also widened from
  `noundef` alone to the three attributes upstream accepts. Each only adds
  `true` answers where upstream proves `true`; none can turn a `true` into a
  `false`. Only the `@llvm.assume` arm and the pointer-cast strip remain, and
  both are recorded in the parity ledger.

### ValueTracking: the select-pattern vocabulary (tranche 4a)

#### Added

- `select_pattern.rs`: `SelectPatternFlavor`, `SelectPatternNaNBehavior`,
  `SelectPatternResult`, `get_select_pattern`, and the flavour accessors
  `min_max_predicate`, `min_max_intrinsic`, `inverse_min_max` and
  `min_max_limit`.
- `MinMaxIntrinsic` — the four integer min/max intrinsics as a public enum,
  because llvmkit's intrinsic semantic is crate-internal and cannot appear in
  a public signature. It is also exactly the range of upstream's
  `getMinMaxIntrinsic`, which makes `MinMaxIntrinsic::inverse` **total** where
  upstream's `getInverseMinMaxIntrinsic` needs an `llvm_unreachable`.

Upstream ends four of these in `llvm_unreachable` on a precondition the caller
must uphold ("caller must ensure `SPF` is an integer min or max pattern").
Each returns `Option` here, so the precondition is the `None` and a caller
cannot read an answer that was never defined.

Matching an actual `select` against these flavours — `matchSelectPattern`,
`matchDecomposedSelectPattern` and the `matchClamp` / `matchMinMax` /
`matchMinMaxOfMinMax` machinery behind them — is tranche 4b and stays in the
gap table. `getInverseMinMaxIntrinsic` is partially modeled: its four integer
arms are `MinMaxIntrinsic::inverse`, while the six floating-point intrinsics it
also inverts (`maximum`/`minimum`, `maxnum`/`minnum`, `maximumnum`/`minimumnum`)
have no counterpart in llvmkit's intrinsic model to map to.


### KnownBits: the operators

An audit of `KnownBits.h`'s public surface against llvmkit — 99 named members
plus 7 operators, enumerated by hand — found two genuinely unmodeled:
`operator<<=` and `operator>>=`.

#### Added

- All seven operators, each as the std trait that spells the same thing:
  `Shl<u32>` / `ShlAssign<u32>`, `Shr<u32>` / `ShrAssign<u32>`, and
  `BitAnd`/`BitOr`/`BitXor` with their `*Assign` forms over the existing
  `bitand`/`bitor`/`bitxor`. `operator==`/`!=` were already covered by the
  derived `PartialEq`/`Eq`.

  The two shifts are **not** aliases for `KnownBits::shl` / `lshr`. Those are
  the transfer functions for the shift *instructions* and know the vacated
  bits are zero; the operators move the masks only, so a shifted-in bit is
  clear in both masks — the encoding of "unknown". LLVM keeps the same pair
  and its callers depend on the difference: `RISCVISelLowering.cpp` follows
  `Known <<= ShAmt` with `Known.Zero.setLowBits(ShAmt)` and the comment "the
  `<<=` operator left these bits unknown".

#### Fixed

- The parity ledger was missing a row for `getConstant` (modeled all along as
  `constant`), and listed no operators at all — the grep that built it looked
  for an identifier before a `(`, which `operator<<=(` does not have. That is
  how the two missing shifts went unnoticed while
  `known_bits_public_surface_is_complete` reported the surface closed.

  That test's doc comment now says what it can and cannot prove: it compares
  two hand-maintained tables and cannot read `KnownBits.h`, which is gitignored
  and absent in CI, so it detects a *recorded* gap and never an unrecorded one.
  Closing the surface is a periodic manual enumeration;
  `KNOWN_BITS_SURFACE_AUDITED` records when it last ran and against what count.

### Parser: vector integer arithmetic and comparison

`and <2 x i32> %a, %b` now parses. It did not before: the parser converted both
operands to `IntValue`, whose `IntWidth` marker describes a scalar width, so
every integer binop and `icmp` rejected vector operands. `clang -O2` output was
unparseable as soon as the vectorizer fired.

#### Added

- `IRBuilder::build_int_binop_erased` — an integer binop on erased operands
  with a *runtime* opcode, for callers like the `.ll` parser that do not know
  the opcode statically. Returns an erased `ValueId<B>`, since the result may
  be a vector.
- `IRBuilder::build_int_cmp_erased` — `icmp` on erased operands, yielding `i1`
  or `<N x i1>` to match. `build_int_cmp_with_flags_dyn` cannot serve: its
  `_dyn` means dynamic *width*, it routes through the scalar-only
  `IntoIntValue`, and its `IntValueId<bool, B>` result cannot describe a vector.
- `IntBinOpFlags`, carrying all four integer-binop flags at once, plus
  `BinaryOpcode::accepted_flags` to drop the ones an opcode cannot express.
- `build_int_udiv_dyn`, `build_int_sdiv_dyn`, `build_int_urem_dyn`,
  `build_int_srem_dyn` — the erased family had stopped at the shifts.

#### Changed

- The erased integer-binop path now **validates** operand types instead of
  leaving it to `verify()`. A caller reaching it has a runtime type in hand and
  no conversion to bounce off, so `and` on two floats used to build silently.
- The erased builders now carry flags. They previously built
  `BinaryOpData::new(..)` with every flag false, so routing the parser through
  them as-written would have made `add nuw <2 x i32>` print as a plain `add`.
- `haveNoCommonBitsSet` recognises a vector `not`: `m_AllOnes` matching was
  scalar-only, so `xor %v, splat (i32 -1)` was not seen as a `not` and the
  `(A & B) op ~(A | B)` case failed on vectors.

The scalar path is untouched — vectors are routed around the typed handles,
not through a relaxed version of them.

### ValueTracking: value-level predicates — tranche 1 complete

The predicates that sit directly on known bits. No new types; the tranche was
listed first in the plan and is now closed.

#### Added

- Sign predicates: `is_known_non_negative`, `is_known_negative`,
  `is_known_positive`.
- `masked_value_is_zero`, which takes the mask upstream's `MaskedValueIsZero`
  takes. The parity ledger previously mapped that name to `is_known_zero`,
  which takes none; `is_known_zero` and `is_known_one` are now recorded as
  llvmkit conveniences with no upstream entry point of their own.
- `is_sign_bit_check`, returning `Option<bool>`. Upstream returns `bool` and
  writes the polarity through a `bool &TrueIfSigned` out-parameter, so the
  polarity can be read after a failed classification; here it cannot be.
- `is_known_to_be_a_power_of_two`, including `isPowerOfTwoRecurrence`.
- `is_known_non_equal`, with `getInvertibleOperands` and its five helpers.
- `is_known_negation`, `is_known_inversion`.
- `is_only_used_in_zero_comparison` and `is_only_used_in_zero_equality_comparison`.
- `have_no_common_bits_set`, with all six `haveNoCommonBitsSetSpecialCases`
  patterns, each tried in both operand orders.
- `is_known_not_undef` and `is_known_not_undef_or_poison`.

#### Changed

- **`is_known_not_poison` is substantially stronger.** It was a placeholder
  that handled constants and shift operators and answered `false` for
  everything else; it is now the real `isGuaranteedNotToBeUndefOrPoison` walk —
  `noundef`/`dereferenceable` parameters, allocated objects, `freeze`, calls
  with a `noundef` return, and the operand walk under
  `canCreateUndefOrPoison`. Anything built on it (`implies_poison`, the
  `freeze` arm of `compute_known_bits`) gets the improvement for free.

Three of the walk's arms are deferred, each only weakening the answer:
`programUndefinedIfUndefOrPoison` and the dominating branch-condition walk
that follows it (CFG reachability, tranche 6), the `@llvm.assume` arm
(tranche 8), and `stripPointerCastsSameRepresentation` before the
allocated-object test (tranche 5). One arm is a deliberate llvmkit refinement,
marked at its site: a shift whose amount known bits prove in range is not
poison, where upstream's `shiftAmountKnownInRange` demands a literal constant.

#### Found

- **The parser cannot read vector integer binops the builder can write.**
  `and <2 x i32> %a, %b` does not parse: `parse_int_binop` converts both
  operands to the scalar-only `IntValue` before calling a builder. The
  type-erased `build_int_*_dyn` family handles vectors fine, so this is a
  parser routing gap, not a missing capability. It surfaced while porting the
  third block of upstream's `HaveNoCommonBitsSet` test, which is therefore not
  ported. Not fixed here — closing it also needs four `_dyn` builders that do
  not exist yet (`udiv`, `sdiv`, `urem`, `srem`). Recorded in
  `docs/future-work.md`.

### ValueTracking: constant ranges and overflow prediction (slice 3e) — tranche 3 complete

The consumers `ConstantRange` was ported for.

#### Added

- `compute_constant_range` and `compute_constant_range_including_known_bits`.
  The two sources are independent — known bits pin bits a range cannot
  express, a range bounds values the bits cannot — so upstream intersects
  them, and so does this.
- All six overflow predicates: `compute_overflow_for_{signed,unsigned}_{add,sub,mul}`.
  The signed multiply reaches its answer by sign-bit counting rather than
  ranges (upstream credits *Hacker's Delight*), which is why slice 1's
  `compute_num_sign_bits` was a prerequisite.
- Five `ConstantRange` overflow predicates the earlier survey missed:
  `unsigned_add_may_overflow`, `signed_add_may_overflow`,
  `unsigned_sub_may_overflow`, `signed_sub_may_overflow`,
  `unsigned_mul_may_overflow`, plus the shared `OverflowResult`.

  **A correction:** `ConstantRange`'s public surface is **83** methods, not
  the 78 reported when tranche 3 was planned. These five span two lines in the
  header, and the extraction used to count them was single-line.

Not ported, each because llvmkit lacks the *input* rather than the reasoning:
`computeConstantRange`'s `@llvm.assume` refinement (no `AssumptionCache`), its
select-pattern clamp (no `SelectPatternResult` — tranche 4), and
`getVScaleRange`, which reads `vscale_range`'s packed `(min, max)` pair while
llvmkit's attribute payload is a single `u64` and `attribute_td_drift.rs`
already lists the attribute as not-yet-modeled. Each omission only widens an
answer.

**Tranche 3 is complete.** `ConstantRange` went from a 223-line seed with 13
mapped methods to 90 public methods across five slices.

### `ConstantRange` dispatchers and no-wrap variants (slice 3d-vi) — slice 3d complete

#### Added

- `binary_op` and `overflowing_binary_op`, the opcode dispatchers.
- `intrinsic` and `is_intrinsic_supported`, plus the `RangeIntrinsic` enum.
- `add_with_no_wrap`, `sub_with_no_wrap`, `multiply_with_no_wrap`, taking a
  `NoWrapKind` with named `signed` / `unsigned` flags rather than upstream's
  bitmask, so neither promise can be mistaken for the other at a call site.
- `smul_fast` — the give-up-rather-than-approximate multiply.

Two places where naming a type removed a run-time check. Upstream's
`intrinsic` asserts that the id is supported *and* that the flag operands are
known one-bit constants; `RangeIntrinsic` carries each intrinsic's flag in the
variant, so both assertions become unrepresentable states and
`is_intrinsic_supported` is total. Arity is still checked, but answers `None`
instead of asserting.

`shlWithNoWrap` is **not** ported — it is three sizeable helpers plus a
dispatcher, no llvmkit caller needs it, and `overflowing_binary_op` sends
`shl` to the plain `shl`, which is sound and only weaker. Recorded in
`docs/future-work.md`.

With this, **slice 3d is complete** — all six sub-slices, ~37 methods.

### `ConstantRange` saturating operations and bit counting (slices 3d-iv, 3d-v)

#### Added

- The saturating family: `uadd_sat`, `sadd_sat`, `usub_sat`, `ssub_sat`,
  `umul_sat`, `smul_sat`, `ushl_sat`, `sshl_sat`. Six of the eight are the
  same shape — monotone, so the extremes pair up — and share one private
  frame rather than being written out separately. `smul_sat` needs all four
  corner products for the same reason `multiply` does, and `sshl_sat` picks
  each endpoint's shift amount by that endpoint's sign, because shifting a
  negative value left drives it down.
- Bit counting: `ctlz`, `cttz`, `ctpop`, with the `zero_is_poison` flag the
  `llvm.ctlz` / `llvm.cttz` intrinsics carry.

A note for anyone writing tests against the saturating shifts: upstream's
`ushl_ov` / `sshl_ov` open with `Overflow = ShAmt >= getBitWidth()` *before*
looking at the value, so `0 ushl_sat 4` at four bits saturates to the maximum
rather than staying zero. The naive `0 << 4 == 0` reading is wrong, and the
test oracle says so explicitly.

### `ConstantRange` bitwise operations and shifts (tranche 3, slice 3d-iii)

#### Added

- `binary_and`, `binary_or`, `binary_xor`, `binary_not`.
- `shl`, `lshr`, `ashr`.

`binary_and` and `binary_or` each intersect two independent approximations —
what the known bits say, and an interval derived from the ranges' extremes —
because neither subsumes the other. `binary_or` reuses the AND's lower-bound
estimator on complemented operands, which is De Morgan applied to the bound
rather than to the operation.

#### Fixed

- **`ApInt::ashr` filled with zero instead of the sign bit** when the shift
  amount reached or exceeded the bit width. Upstream's `APInt::ashrInPlace`
  spells that arm `SExtVAL >> (APINT_BITS_PER_WORD - 1)` — "fill with sign
  bit" — so a negative value must saturate to all-ones, not zero. The `APInt`
  overload reaches it via `getLimitedValue(BitWidth)`, so larger amounts
  saturate the same way.

  `ConstantRange::ashr` surfaced it: the range for `-8 ashr 4` at four bits
  dropped `-1`. `checked_ashr` keeps its stricter contract and still declines
  out of range; only the saturating wrapper changed.

### `ConstantRange` division and remainder (tranche 3, slice 3d-ii)

#### Added

- `udiv`, `sdiv`, `urem`, `srem`.
- `abs`, pulled forward from slice 3d-v because `srem` reasons about the
  divisor's magnitude.

Two undefined-behaviour cases shape these. Division and remainder by zero is
UB, so a divisor range that can *only* be zero yields the empty set, and one
that merely contains zero has the zero skipped when picking the smallest
divisor. `SignedMin / -1` is UB at the IR level even though `APInt` defines it
— upstream's `neg / neg` arm computes its bound twice, once dropping `-1` from
the divisor and once dropping `SignedMin` from the dividend, and llvmkit
carries that over.

`sdiv` splits both operands by sign and computes the four sign combinations
separately, since the quotient's sign follows from the operands' and mixing
them loses precision. The zero that the split drops from the dividend is
unioned back at the end.

The tests skip exactly the undefined pairings and no others — a range analysis
is only obliged to cover defined behaviour, but skipping more than that would
let a wrong answer through.

### `ConstantRange` arithmetic, first group (tranche 3, slice 3d-i)

Slice 3d is ~30 methods across six unrelated families, so it is cut into
sub-slices the same way tranche 3 was (see `docs/future-work.md`). This is the
first: the operations `computeOverflowFor*` needs in 3e.

#### Added

- `add`, `sub` — endpoint arithmetic with upstream's wrap detection: a result
  smaller than either input can only mean the true answer covers everything.
- `multiply` — computed at double width under both an unsigned and a signed
  reading, returning the smaller. Multiplication is signedness-independent but
  the resulting *range* is not, and neither reading dominates.
- `smax`, `smin`, `umax`, `umin`.

The four min/max functions are near-identical upstream. llvmkit writes the
body once over a private `MinMaxKind` naming the two axes they differ on
(signed vs unsigned, min vs max), so they cannot drift apart.

### `ConstantRange` ICmp regions (tranche 3, slice 3c)

#### Added

- `make_allowed_icmp_region` — values that compare true against **some**
  member of a range (an over-approximation).
- `make_satisfying_icmp_region` — values that compare true against **every**
  member (an under-approximation). Derived from the allowed region by
  De Morgan, as upstream does.
- `make_exact_icmp_region` — exact, because a single right-hand value makes
  the two questions above coincide.
- `make_mask_not_equal_range` — the values satisfying `(v & mask) != c`.
- `icmp` — true when every pairing across two ranges compares true.
- `equivalent_icmp_with_offset` / `equivalent_icmp`, porting the two
  `getEquivalentICmp` overloads. Upstream fills out-parameters and returns a
  `bool` for "the offset was zero"; llvmkit returns an `EquivalentICmp` struct
  and an `Option` respectively.
- `single` and `contains_range`, the one-element constructor and the
  range-in-range query these needed.

The signedness-flipping helpers are not ported — they exist upstream for
InstCombine's predicate canonicalization, which llvmkit does not have. Noted
in `docs/future-work.md`.

### `ConstantRange` set operations (tranche 3, slice 3b)

#### Added

- `intersect_with` / `union_with`, ports of `ConstantRange::intersectWith` and
  `unionWith` including their full wrapped/non-wrapped case analysis, plus
  `PreferredRangeType` (`Smallest` / `Unsigned` / `Signed`) — upstream's
  tie-breaker for when the exact answer needs two disjoint runs and only one
  `[lower, upper)` can be returned.
- `inverse`, `difference`, `subtract`, `split_pos_neg`.
- The width-changing family: `zero_extend`, `sign_extend`, `truncate`,
  `zext_or_trunc`, `sext_or_trunc`.

Upstream asserts on a width change in the wrong direction
(`assert(SrcTySize < DstTySize && "Not a value extension")`). llvmkit has no
runtime asserts in production paths, so `zero_extend`, `sign_extend` and
`truncate` return `IrError::OperandWidthMismatch` instead; `zext_or_trunc` and
`sext_or_trunc` are the spellings that accept either direction.

`truncate`'s `NoWrapKind` parameter is spelled as a plain `no_unsigned_wrap:
bool`, because `TruncInst::NoUnsignedWrap` is the only kind the function reads.

`castOp` is deliberately not ported — see `docs/future-work.md`.

### `ConstantRange` bounds and predicates (tranche 3, slice 3a)

Tranche 3 of the `ValueTracking.h` port is `ConstantRange` itself, which
llvmkit had only as a 223-line seed — 13 of upstream's 78 public methods. It
is cut into five slices (see `docs/future-work.md`); this is the first.

#### Added

- Signed bounds: `signed_min`, `signed_max`, `is_sign_wrapped_set`,
  `is_upper_sign_wrapped`.
- Membership shape: `single_element`, `single_missing_element`,
  `is_single_element`, `is_size_strictly_smaller_than`, `is_size_larger_than`.
- Sign predicates: `is_all_negative`, `is_all_non_negative`,
  `is_all_positive`. The empty set is vacuously all-negative *and*
  all-positive, as upstream documents.
- Width queries: `active_bits`, `min_signed_bits`.
- `non_empty` — the constructor that reads equal endpoints as the full set,
  mirroring `getNonEmpty`.
- `from_known_bits` / `to_known_bits`, bridging `ConstantRange` and
  `KnownBits` in both directions.

#### Fixed

- **`ConstantRange::new` accepted a range that cannot exist.** Equal endpoints
  encode the two degenerate sets — all-zero is empty, all-ones is full — so
  any *other* equal pair describes a range containing nothing while answering
  `false` to both `is_empty_set` and `is_full_set`, which every predicate
  downstream reads. Upstream asserts against it in
  `ConstantRange::ConstantRange`; llvmkit silently built it. The constructor
  now returns the new `IrError::DegenerateConstantRange`. This is a behaviour
  change on a constructor that was already fallible.

### ValueTracking: the poison predicates (tranche 2)

Second tranche of the `ValueTracking.h` port. The value-level poison
reasoning is now modeled; what remains of the family is the CFG walk that
decides whether poison actually *reaches* undefined behaviour.

#### Added

- `can_create_poison` / `can_create_undef_or_poison` — ports
  `llvm::canCreatePoison` and `llvm::canCreateUndefOrPoison`, including the
  `ConsiderFlagsAndMetadata` parameter. An operator carrying `nsw`, `nuw`,
  `exact`, `disjoint`, `nneg` or a gep no-wrap flag answers true on that
  basis alone.
- `propagates_poison` — ports `llvm::propagatesPoison`. Upstream takes a
  `Use`, which names a user and an operand position together; llvmkit has no
  `Use` type, so the pair is spelled out as `(user, operand_index)`.
- `implies_poison` — ports `llvm::impliesPoison`, with upstream's local
  recursion limit of 2 (not the analysis-wide depth).
- `is_known_not_poison` — makes the already-internal
  `isGuaranteedNotToBePoison` public.

Two upstream arms are not ported and each only weakens an answer: the
`nocreateundeforpoison` function attribute, which llvmkit does not model, so
some calls answer "can create poison" where upstream would not; and
`directlyImpliesPoison`'s `extractvalue`-of-`WithOverflowInst` arm, whose
pattern cannot arise because llvmkit does not model the overflow intrinsics
as a distinct instruction class.

### ValueTracking: sign-bit counting (tranche 1)

First tranche of the `ValueTracking.h` port. Ports
`llvm::ComputeNumSignBits` and `llvm::ComputeMaxSignificantBits`
(`ValueTracking.cpp`) — the number of times a value's sign bit is replicated
into its high bits, and the signed width that implies.

#### Added

- `compute_num_sign_bits` — ports `ComputeNumSignBitsImpl`'s operator switch:
  `sext`, `trunc`, `sdiv`/`srem` by a positive constant, `ashr`, `shl`,
  `and`/`or`/`xor`, `select`, `add`, `sub`, `mul` and `phi`, then the
  `computeKnownBits` tail that improves on whatever the switch established.
- `compute_max_significant_bits` — `width - sign_bits + 1`.

Four upstream arms are **not** ported, each falling through to the
known-bits tail exactly as upstream's own `break` does when its pattern fails
— weaker, never wrong. They are the vector arms (`BitCast` across element
widths, `ShuffleVector`, `ExtractElement`), which read
`getShuffleDemandedElts`; the `shl`-through-`zext` look-through and the
`select` signed min/max clamp, which need the matcher DSL; and the `abs` /
`smin` / `smax` intrinsic arms.

Upstream asserts its result is never zero; llvmkit enforces that floor
instead, so a zero cannot escape into the arithmetic that subtracts from it.

### `KnownBits` models all of `KnownBits.h`

Three operations closed the last of the public-surface gap the parity ledger
found:

#### Added

- `KnownBits::set_all_ones` — makes every bit known-one, discarding prior
  information. Mirrors `KnownBits::setAllOnes`, the dual of the existing
  `set_all_zero`.
- `KnownBits::is_sign_unknown` — true when the sign bit is not known either
  way. Mirrors `KnownBits::isSignUnknown`. It reads the masks directly rather
  than being spelled `!is_negative() && !is_non_negative()`, because those two
  both answer `false` at width zero, which would wrongly report a value with no
  sign bit as sign-unknown.
- `KnownBits::sdiv` — the two-argument spelling, `sdiv_with_exact(.., false)`,
  matching the `udiv` / `udiv_with_exact` pair. Upstream has one `sdiv` with
  `Exact` defaulted; Rust has no default arguments, so llvmkit spells both.

The ledger now asserts `KnownBits.h` has no remaining gaps. Two entries it
previously listed — `flipSignBit` and `remGetLowBits` — were recorded in error:
both are **private** in `KnownBits.h`, and both already existed in llvmkit as
module-private helpers.

### The KnownBits / ValueTracking parity ledger is real

`tests/value_tracking_parity.rs` claimed to be the coverage ledger for the
known-bits surface. Its one test asserted that a `const` array contained the
same strings it had just been initialised with — it could not fail, and it had
never recorded any coverage.

It now tabulates the surface: 93 of the 98 `KnownBits.h` operations llvmkit
models, mapped upstream-name to llvmkit-name, plus the five it does not and
the reason for each; and the four `ValueTracking.h` entry points modeled
against the nine families that are absent wholesale. The modeled columns are
held to the crate by *calling* every entry, so a rename or deletion stops the
file compiling. The tables are checked for the properties a ledger needs to
stay readable, and the gap lists record the LLVM release they were derived
from.

Building it surfaced five `KnownBits` operations with no llvmkit counterpart —
`setAllOnes`, `flipSignBit`, `isSignUnknown`, `remGetLowBits`, and the
`sdiv` / `sdiv_with_exact` spelling asymmetry. None is load-bearing for
anything llvmkit computes today; all five are recorded in
`docs/future-work.md`.

Because `orig_cpp/` is gitignored, the gap lists cannot be re-derived at test
time and stay a hand-maintained record — the file says so rather than implying
otherwise.

### View iterators no longer borrow the view

`function.basic_blocks().flat_map(|block| block.instructions())` — the obvious
way to walk every instruction in a function — did not compile. It failed with
E0515, "cannot return value referencing function parameter `block`", and the
workaround was a nested loop with a labeled break.

The iterator never held anything belonging to the block: it snapshots the
instruction ids and copies the `ModuleRef`. But edition 2024 captures every
lifetime in scope in a return-position `impl Trait`, including the `&self` the
method was called on, so the compiler believed otherwise. The affected returns
now carry a precise-capturing `use<..>` bound that leaves that lifetime out.

Fixed on `BasicBlock::instructions`, `BasicBlockView::instructions`,
`PhiInst`/`FpPhiInst`/`PointerPhiInst`/`OtherPhiInst`/`PhiKind::incomings`,
`SwitchInst::cases`, `IndirectBrInst::destinations`, `LandingPadInst::clauses`,
`CatchSwitchInst::handlers`, and `FnPatch::body_instructions`.

Iterators that genuinely borrow the receiver are unchanged and still do —
`AttributeSet::iter` and `AttributeStorage::iter` yield `&Attribute`,
`Cfg::edges` iterates a borrowed slice, and the `pass_names` family yields
`&str`. Iterators taking `self` by value (`Module::functions`, `globals`,
`aliases`, `ifuncs`, `comdats`, `FunctionValue::basic_blocks`, `params`) never
had the problem.

This is a relaxation: code that compiled before still compiles.

### Known bits reason about loop recurrences

`compute_known_bits` on a `phi` now recognises a simple two-predecessor
recurrence — `%iv = phi [start, %entry], [%iv.next, %backedge]` where
`%iv.next` is a binary operator with `%iv` as an operand — and reads facts off
it, porting the `Instruction::PHI` arm of `computeKnownBitsFromOperator` and
`matchSimpleRecurrence` (`ValueTracking.cpp`).

A `shl` recurrence keeps the start value's trailing zeros; `lshr`, `udiv` and
`urem` keep its leading zeros; `ashr` extends its sign bit in both directions;
and `add`/`sub`/`and`/`or`/`mul` keep the trailing zeros common to the start
and the step, plus the `nsw` sign facts. `urem` is the one opcode that accepts
the phi on either side. Previously a phi answered only with the intersection
over its incoming values, which a loop backedge leaves unknown.

`llvm/test/Analysis/ValueTracking/recurrence-knownbits.ll` is checked in
verbatim and driven as the test: twelve of its fifteen functions now reproduce
their CHECK line exactly. The three that do not need InstCombine
canonicalization rather than more analysis, and are pinned as gaps —
see `docs/future-work.md`.

#### Added

- `KnownBits::mark_low_bits_zero`, `mark_high_bits_zero` and
  `mark_high_bits_one`, spelling upstream's `Known.Zero.setLowBits` /
  `setHighBits` / `Known.One.setHighBits`.

### Constants are uniqued

Four constant kinds were minting a fresh arena node on every request:
`GlobalValueRef`, `GepOffset`, `SymbolDelta`, and `SymbolDeltaPlus`. Every
other kind — ints, floats, null, undef, poison, aggregates, expressions,
block addresses, `dso_local_equivalent`, `no_cfi`, `none`, `ptrauth` — already
uniqued. The four now key on their structural fingerprint the same way, so
llvmkit matches LLVM's rule that two structurally equal constants are one
node.

#### Fixed

- **Identity comparison on constants under-folded.** Folds ported from
  upstream write `A == B` on two constants because LLVM uniques `Constant*`.
  Where llvmkit did not unique, those arms were unreachable for
  independently-built operands and the fold quietly declined — sound (an
  `==` that says *true* still implies the same value), but weaker than
  upstream. The affected arms were `ConstantFoldSelectInstruction`'s
  `V1 == V2`, `fold_phi`'s cross-incoming comparison, `constant_splat_value`,
  and the pointer-base comparison in `ConstantFoldCompareInstOperands`.
- **`base_identity` is retired.** It was the workaround that resolved a
  `GlobalValueRef`/`GepOffset` wrapper down to the underlying global so the
  pointer-base fold could compare bases at all. With uniquing, the base
  comparison is plain `==` — exactly upstream's `Stripped0 == Stripped1`.

#### Note

Forward `blockaddress` placeholders are deliberately **not** uniqued. They
carry no payload, so uniquing would collapse every pending forward reference
in a module into one node and resolving the first would resolve them all. The
constructor is named `push_constant_block_address_placeholder`, not `intern_`,
for that reason.

llvmkit uniques **per module** rather than per context, because a `Module`
owns its `Context`. Two modules never share a constant node; a foreign id is
caught by the module tag check rather than silently matching.

### `ApInt` sweep complete; sentinels become sum types

Finishes `llvm/unittests/ADT/APIntTest.cpp`: sixty-seven more ported tests
covering the wide-division paths, the comparison family, the bit-set and
bit-clear families, saturating and overflow-flagged arithmetic, and the
operation families llvmkit had never modeled. The division paths — the least
covered code in `ap_int.rs` — were already bit-exact.

#### Fixed

- **`ApInt::is_one_bit_set` carried upstream's name over a different meaning.**
  It answered "bit N is set", where `APInt::isOneBitSet` means "bit N is set
  **and is the only bit set**". No in-tree caller was wrong — all seventeen
  wanted plain bit access — but a port of LLVM code calling `isOneBitSet` would
  have silently accepted values with other bits set. The callers now say
  `bit()`, and `is_one_bit_set` means what upstream means.

#### Changed (breaking)

- **`ApFloat::ilogb` and `ApFloat::frexp` return `BinaryExponent`, not `i32`.**
  Upstream reserves three `int` values as markers — `IEK_NaN` is `INT_MIN`,
  `IEK_Zero` is `INT_MIN + 1`, `IEK_Inf` is `INT_MAX` — so a caller that forgets
  to test for them does arithmetic on `INT_MIN` and the type system cannot tell
  that apart from a real exponent. The new sum type has `Finite(i32)`, `Zero`,
  `Infinity`, and `Nan`, so the exponent exists only where it means something.
  The `ILOGB_NAN` / `ILOGB_ZERO` / `ILOGB_INF` constants are gone.
- **`ApInt::log_base2`, `nearest_log_base2`, and `exact_log_base2` return
  `Option<u32>`** for the same reason: upstream's zero answer is an underflowed
  `unsigned`, and `exactLogBase2`'s "not a power of two" answer is `-1`.

#### Added

`ApInt` gains the operations the sweep found missing, each ported from its
upstream definition and covered by that definition's test: `bit` (upstream's
`operator[]`), `set_bits`, `clear_bits`, `clear_low_bits`, `clear_high_bits`,
`set_all_bits`, `clear_all_bits`, `flip_all_bits`, `flip_bit`, `rotl`, `rotr`,
`rotl_by`, `rotr_by`, `lo_bits`, `hi_bits`, `is_splat`, the `log_base2` family,
`mul_high_signed`, `mul_high_unsigned`, `rounding_udiv`, `rounding_sdiv`, the
four `avg_*` averages, `abs_diff_signed`, `abs_diff_unsigned`,
`multiplicative_inverse`, `greatest_common_divisor`,
`most_significant_different_bit`, `pow`, `scale_bit_mask`, `fshl`, `fshr`, and
the three carry-less multiplies. Comparison gains four total orderings —
`unsigned_cmp`, `signed_cmp`, `unsigned_cmp_u64`, `signed_cmp_i64` — which
answer all sixteen of upstream's comparison overloads, including the ones
against a machine word that llvmkit previously could not express at all.

### ApFloat audit complete; first `ApInt` sweep

Closes the ApFloat families — `FMA`, `roundToIntegral`, `toInteger` — and
opens `llvm/unittests/ADT/APIntTest.cpp`, which had never been swept.
`FMA` and `roundToIntegral` were already bit-exact.

#### Fixed

- **`ApFloat::convert_to_integer` returned zero instead of saturating.** A
  conversion that cannot fit reports invalid-op *and* fills the destination
  with the extreme of the target type — the `opInvalidOp` tail of
  `IEEEFloat::convertToInteger`. A `double` of `32` converted to a 5-bit
  unsigned now yields `31`, not `0`; a NaN still yields zero, and a negative
  signed overflow yields the minimum.
- **`ApInt::is_mask` answered `true` for zero.** It compared the trailing-ones
  count against the active-bit count, so `0 == 0` made every zero a mask.
  Upstream requires a non-empty run (`isMask_64` leads with `Value &&`, and the
  multi-word path requires `Ones > 0`).
- **`ApInt::to_string_radix` emitted lower-case hexadecimal.**
  `APInt::toString` defaults to `UpperCase = true`. Only radix 10 reaches the
  IR printer, so printed IR is unchanged.

### ApFloat audit: `ilogb`'s three dependants

Ports of `TEST(APFloatTest, scalbn)`, `frexp`, `getExactLog2`, `next`,
`remainder` (its 340-row table plus the individual cases), and `mod`. The
`remainder`, `mod`, and `getExactLog2` families were already bit-exact; the
other three were not.

#### Fixed

- **`scalbn` left a signaling NaN signaling.** Upstream's last act is
  `if (X.isNaN()) X.makeQuiet()`, so the result is quiet with its payload
  intact.
- **`frexp` reported an exponent of `0` for NaNs and infinities**, and did not
  quiet a signaling NaN. Upstream opens with `Exp = ilogb(Val)` and returns
  before overwriting it, so the answer for those categories is `ilogb`'s
  sentinel — `ILOGB_NAN` and `ILOGB_INF`, not zero. Only a zero answers `0`.
- **`next` quieted a signaling NaN in place**, keeping the payload and the
  "make it a NaN, not an infinity" filler bit. Upstream builds a *fresh*
  payload-less quiet NaN carrying only the sign
  (`makeNaN(false, isNegative(), nullptr)`). This is a deliberate asymmetry
  with `scalbn` / `frexp` / `convert`, which do quiet in place — worth knowing
  before "simplifying" any of the four to share a helper.

### ApFloat `convert` carries NaN payloads

#### Fixed

- **Converting a NaN between semantics discarded its payload.** The NaN arm of
  `convert` rebuilt a bare quiet NaN in the target semantics, so
  `snan` widened from `float` to `x86_fp80` produced the default pattern
  instead of upstream's payload-carrying one, and no truncation could ever
  report a lost payload. It now ports the `fcNaN` arm of `IEEEFloat::convert`:
  the significand is shifted by the precision difference — right on a
  truncation, where dropped bits make the conversion lossy, left on an
  extension — and the result is always quiet, with a signaling source raising
  invalid-op. That last rule is what keeps a signaling NaN from becoming an
  infinity when a truncation drops every payload bit.

  Reassembly routes back through the NaN constructor rather than laying out
  bits at the call site, so payload masking, the quiet bit, and x87's explicit
  integer bit keep one definition each.

### ApFloat audit, continued: three more defects

Ports of the `APFloat` classification, factory, and small-operation tests from
`llvm/unittests/ADT/APFloatTest.cpp` — twenty upstream `TEST(...)` blocks —
found three more divergences.

#### Fixed

- **`ilogb` answered `0` for almost everything.** It delegated to
  `exact_log2_abs`, which is `None` unless the value is *exactly* a power of
  two, and then defaulted to `0`. So `ilogb(0x1.ffffffffffffep-1023)` reported
  `0` where upstream reports `-1023`, and every NaN, zero, and infinity
  reported `0` as well. It now ports the free function `llvm::ilogb`: the
  unbiased exponent of any finite non-zero value, denormals included, with
  upstream's sentinels exposed as `ApFloat::ILOGB_NAN` / `ILOGB_ZERO` /
  `ILOGB_INF` (`APFloat::IEK_NaN` / `IEK_Zero` / `IEK_Inf`).
- **`ppc_fp128` factories left a negative-zero residual.** `inf`, `smallest`,
  `smallest_normalized`, and the NaN constructors signed the value by flipping
  the whole 128-bit pattern, which negates the residual half too. Upstream's
  `DoubleAPFloat` factories sign only the leading component and set the
  residual with `Floats[1].makeZero(/*Neg=*/false)`. `makeLargest` is the
  deliberate exception — it negates both — and is left alone.

#### Added

- **`ApFloat::is_smallest_normalized`**, which had no counterpart at all
  despite `TEST(APFloatTest, IsSmallestNormalized)` exercising it across every
  semantics. Ports both upstream implementations, using the numeric comparison
  `DoubleAPFloat` uses: a bitwise test would additionally demand the `ppc_fp128`
  residual's sign, where a `(smallest normal, -0.0)` pair counts upstream.

### ApFloat bit-exactness audit (three fixes)

Audited against LLVM's own `llvm/unittests/ADT/APFloatTest.cpp` from the
vendored 22.1.4 tree, not against hand-derived expectations. All 784 rows of
the `add` / `subtract` / `multiply` / `divide` special-case tables now ship as
`llvmkit-ir/tests/ap_float_upstream_arithmetic.rs`.

#### Fixed

- **A signaling NaN lost its payload when it was quieted.** `make_quiet`
  rebuilt a default quiet NaN instead of setting the quiet bit, so
  `snan123 + 1.0` produced a bare `nan` where LLVM produces `nan123`. It now
  ports `IEEEFloat::makeQuiet` exactly — one bit set, sign
  and payload untouched. This was the *only* divergence in the 784-row
  arithmetic matrix: it accounted for all 104 failing rows, and every row not
  involving a signaling NaN already matched bit for bit.
- **`fp128` hex literals parsed with their two 64-bit halves transposed.**
  `0xL…` is not a big-endian 128-bit number: `LLLexer::HexToIntPair` reads the
  first sixteen hex digits into the APInt's *low* word. llvmkit read the whole string big-endian, so
  `0xL00000000000000003FFF000000000000` — LLVM's spelling of 1.0 — was read as
  a subnormal, and printing it back produced a different spelling.
- **`ppc_fp128` printed its two components in the wrong order.** Upstream
  writes the leading double first (`WriteConstantInternal` prints
  `getLoBits(64)` first, and its low word holds `DoubleAPFloat::Floats[0]`).
  Values were never wrong here — llvmkit stores the pair mirrored from upstream
  and the two mirrorings cancelled on the parse side — but the printed text
  disagreed with LLVM, and `parse → print` oscillated between two spellings.

  Two round-trip fixtures asserted the transposed spellings and were updated;
  they had encoded the defect.

#### Known divergence, now recorded and pinned

- `ApFloat::to_bits` does **not** agree with upstream's `bitcastToAPInt` for
  `PpcDoubleDouble` alone: llvmkit keeps the leading double in the high word
  where upstream keeps it in the low word. Invisible to finite arithmetic
  (llvmkit sums both components) and invisible in the textual form (reader and
  printer both compensate); visible only to a raw-bit reader. Pinned by
  `llvmkit-ir/tests/ap_float_ppc_word_order.rs`.

### Lexer diagnostics name what the lexer choked on

#### Changed

- **BREAKING: `LexError::UnknownToken` gained a `reason` field.** It used to
  render as a bare `invalid token` at every one of its ten construction sites,
  so a misspelled attribute, a stray byte, a `$` with no comdat name, and a
  hexadecimal float prefix with no digits were indistinguishable. Each now
  reports its own message: `unknown keyword 'nocalback'`, `no token starts with
  '\x01'`, `expected a comdat name after '$'`, `expected hexadecimal digits
  after '0xK'`, `'.' is a token only as part of '...'`, and so on. Callers that
  matched `LexError::UnknownToken { span }` need `{ span, .. }`; the new
  `UnknownTokenReason` is a public enum, so the reason can be matched on rather
  than parsed out of a string.

  This is a deliberate improvement on upstream, not a port. `LLLexer` records
  no message at any of these sites — it returns a bare `lltok::Error` and lets
  `LLParser` describe the failure from the surrounding production. That is
  adequate when the parser is always the caller; llvmkit's lexer is public, so
  its errors have to stand on their own.

- The span reported for an unknown *keyword* now covers the whole word instead
  of its first byte. The cursor rewind that produced the one-byte span is
  upstream behaviour (`LLLexer::LexIdentifier`) and is unchanged — only the reported
  span moved, because a caret under the `n` of `nocalback` helps nobody.

### Parser — Milestone 0 complete

#### Added

- **Aliases and ifuncs may name a target declared later in the file.** The
  printer emits them before function declarations, so a printed module whose
  ifunc resolver is a declared function previously failed to re-parse. Forward
  targets now become a placeholder patched at end of module, mirroring the
  mechanism `personality` already used. Backward references and genuinely
  undefined targets are unchanged.
- **An anti-drift guard for the attribute keyword table.** `Attributes.td` is
  vendored under `llvmkit-asmparser/tablegen/`, and a test asserts every LLVM
  22.1.4 attribute is either accepted by the parser in a position the `.td`
  declares for it, or named in an explicit `NOT_YET_MODELED` list. A new
  upstream attribute, or one the parser silently stops accepting, now fails
  CI — the failure mode that hid the ~21 missing keywords in the first place.

#### Fixed

- A global with declaration linkage (`external`, `extern_weak`) carrying an
  initializer reported `expected top-level entity`, pointing at the wrong
  construct entirely. It now reports that a global with that linkage is a
  declaration and takes no initializer.
- Two parser messages read wrong through the `expected {}` frame: a doubled
  `expected expected comma after …`, and a capitalised sentence
  (`An alias or ifunc must have pointer type`) wedged into the fragment slot.

### Parser — ordinary `clang` output now parses (Milestone 0, keyword slice)

#### Added

- **The ~21 attribute keywords real compiler output uses**: `uwtable` (with
  the kind grammar — bare means async, `uwtable(sync)` round-trips),
  `norecurse`, `hot`, `inlinehint`, `sanitize_address`, `ssp`, `sspstrong`,
  `sspreq`, `nonlazybind`, `minsize`; parameter attributes `byval(T)`,
  `sret(T)`, `byref(T)`, `inalloca(T)`, `elementtype(T)`,
  `dereferenceable(N)`, `dereferenceable_or_null(N)`, `inreg`, `nest`,
  `swiftself`; and `captures(none)`, mapped to the modeled `nocapture` (other
  capture components are a pinpointed error, never a silent drop).
- **`dso_local` / `dso_preemptable` on global variables, aliases, and
  ifuncs** — stored, printed in upstream `printGlobal` order, and settable via
  the same getter/setter surface every other global property has. Previously
  the specifiers were accepted on `define`/`declare` only, which alone
  rejected plain `clang` output.
- **`c"..."` string-constant initializers** (`@.str = ... c"hello\0"`),
  which the printer already emitted but the parser could not read back.
- The probe matrix that found all of this ships as
  `tests/parser_attribute_matrix.rs`, including clang-shaped `-O0` and `-O2`
  whole programs asserted to parse, verify, and round-trip.

#### Fixed

- The attribute Displays printed parameter alignment as `align(4)`; the
  grammar everywhere — including our own parser — is `align 4`. All three
  print sites now emit the space form.
- `alignstack` parsed a space form the printer never produced; it now uses
  the upstream paren form `alignstack(N)` end to end.

## [0.0.4] — unreleased

The id-first redesign. Cycles A–E reshaped the core currency of the API —
handles became storable ids, the module became an owned value, and its identity
moved from a lifetime to a type — so almost every construction call site
changes. The migration is mechanical, and each break is spelled out under the
cycle that made it.

The version stays `0.0.4`: that is what the workspace already carried, it was
never published (crates.io has 0.0.3), and under Cargo's pre-1.0 rules every
`0.0.x` is mutually incompatible anyway, so a break needs no wider signal. A
minor bump would imply a stability this crate does not yet have.

The headline shape, for a reader arriving cold:

```rust
let m = module_new!("demo")?;                       // owned, Send, no lifetime
let f = m.add_typed_function::<i32, (i32, i32), _>  // declarations return ids
    ("add", Linkage::External)?;
let entry = m.view(f).append_basic_block(&m, "entry");   // ids resolve to handles
```

### Bare brands — `ModuleBrand` drops its supertraits

#### Changed (breaking-loosening)

- **A brand is now a bare unit struct.** `ModuleBrand`'s supertraits
  (`Copy + Debug + Eq + Hash`) are gone; the trait is `'static` only, so
  `struct LiftedBin; impl ModuleBrand for LiftedBin {}` is a complete brand
  declaration. Every brand that compiled before still compiles — this is a
  loosening — but generic code that *relied* on receiving those impls through
  a `B: ModuleBrand` bound must now ask for them explicitly.
- The supertraits existed because ~100 brand-generic container types used std
  `#[derive]`, which bounds every type parameter whether or not a field uses
  it. Those containers now use **`#[derive(Branded)]`** from `llvmkit-macros`:
  the same impls with the item's generics copied verbatim and no inferred
  bounds. `PartialEq`/`Hash` are generated from one shared field walk (the
  contract cannot drift), and a wrong `Copy` is still rejected by the compiler
  (`E0204`, locked by a compile-fail fixture).
- **`llvmkit-macros` is now a required dependency of `llvmkit-ir`** (and of
  `llvmkit-asmparser`). A proc-macro crate is build-time only — it contributes
  nothing to the built artifact. The `macros` feature still gates exactly what
  it gated before: the user-facing `IrStruct` / `#[function_pass]` /
  `#[module_pass]` re-exports.
- **`Debug` on view types no longer prints phantom fields**, following the
  `decl_value_id!` convention. `#[derive(IrStruct)]`'s generated `<Struct>Value`
  wrapper emits the same bound-free impls inline.
- `module_new!`'s generated brand struct is bare (no derives) — the registry
  keys on `TypeId` alone.

### Packaging

#### Fixed

- **Every published crate now contains the license text.** `LICENSE` lived only
  at the workspace root, and Cargo auto-includes a license file only from the
  *package* directory, so all five `.crate` tarballs shipped with the `license`
  field set and no license in them. For a derivative work of the LLVM Project
  that is a defect rather than an oversight: Apache-2.0 section 4(a) requires
  that recipients of a distribution receive a copy of the License, and a
  crates.io tarball is a distribution. Each package directory now carries a
  verbatim copy of the root `LICENSE`, and CI compares all five against the root
  so they cannot drift.

### The polish and freeze cycle (cycle E)

The API surface is frozen for the release: the remaining asymmetry is closed,
and the documentation is reconciled with the library that cycles A–D actually
produced.

#### Changed (breaking)

- **A plain terminator edge into a parameterised block is now rejected at the
  builder.** Branching into a block created by `append_block_with_params` /
  `append_block_with_named_params` / `append_block_typed` *without* carrying its
  block arguments used to build fine, seed nothing, and leave an incomplete phi
  for a distant `Module::verify()` (`PhiEmptyInReachableBlock`, or the shared
  `check_phi` count guard) to report. Every plain terminator edge now checks its
  target first and returns `IrError::PhiArgArityMismatch` — the same error a
  wrong argument *count* already got from `build_br_with_args`, so one mistake
  reads the same wherever it is caught. Covered: `build_br`, `build_cond_br`,
  `build_switch` / `build_switch_dyn` (default edge) and `SwitchInst::add_case`
  (case edges), all four `invoke` entry points — `build_invoke_with_config`,
  `build_invoke_dyn_with_config`, `build_indirect_invoke_dyn_with_config`,
  `build_inline_asm_invoke_with_config`, and so the `build_invoke` /
  `build_invoke_dyn` / `build_inline_asm_invoke` wrappers over them — on both
  the normal and unwind edge, `build_callbr*` (default and indirect), and
  `IndirectBrInst::add_destination`. The check runs before the terminator is
  emitted, so a rejected edge leaves no half-formed instruction; the builder is
  still consumed, exactly as when a target fails to resolve.

  **What is *not* affected:** the guard keys on "was this block created with
  block parameters", not on "does this block contain phis". A `.ll` back-edge
  into an already-parsed loop header, an `SsaBuilder` back-edge into an unsealed
  header whose reads have minted operandless phis, and a pass-inserted phi are
  all untouched — those phis are completed through their own checked paths and
  their blocks were never declared parameterised. That is also what makes the
  guard free on the hot path: a block records its declared parameter count in
  one `Cell`, and an ordinary target costs that single read rather than a walk
  of its instruction list.

  **Migration:** use the argument-carrying builder for that edge —
  `build_br_with_args` / `build_cond_br_with_args` / `build_switch_with_args` /
  `build_invoke_with_args` (or the `_dyn` / `_call` siblings). `indirectbr`,
  `callbr`, and the indirect-callee and inline-asm-callee `invoke` shapes have
  no argument-carrying form by design — their edges are selected at run time —
  so a parameterised destination is rejected there with no alternative; author
  such a phi through `SsaBuilder` or `FnReshape::insert_phi` on a block created
  with plain `append_basic_block`.
- **Lookups return ids, symmetric with declarations.** `Module::get_global` →
  `Option<GlobalId<B>>`, `get_alias` → `Option<GlobalAliasId<B>>`, `get_ifunc`
  → `Option<GlobalIFuncId<B>>`, `function_by_name_dyn` →
  `Option<FunctionId<Dyn, B>>`, and `function_by_name::<R>` →
  `IrResult<Option<FunctionId<R, B>>>`. Reach the handle with `m.view(id)`,
  exactly as for a declaration's id. The marker check on `function_by_name::<R>`
  is unchanged — a mismatched signature is still
  `IrError::ReturnTypeMismatch`, never a silently widened id. The four
  unconditional lookups also relax `&'ctx self` to `&self`: an id borrows
  nothing, so a lookup no longer pins a borrow of the module.
  `Module::get_comdat` is deliberately exempt and documents why — a comdat is
  not a `Value`, and `ComdatId` is a bare `u32` carrying neither a `ModuleId`
  tag nor a brand, so it is not a member of the id family and `view` cannot
  resolve it.
- **The by-name lookups are state-generic.** `get_global`, `get_alias`,
  `get_ifunc`, and `get_comdat` moved out of the `Module<B, Unverified>` impl
  into the state-generic one, where `function_by_name` / `function_by_name_dyn`
  already lived. They return a capability-free id (or, for comdats, a read-only
  handle), so the `Unverified` restriction bought no safety — it only meant a
  `Module<B, Verified>` had *no* O(1) route to a symbol, leaving a linear scan
  over `as_view().globals()` as the only option. `function_by_name::<R>` also
  relaxes `&'ctx self` to `&self`, matching the other five.
- **`ComdatRef::id` is removed.** It handed out a `ComdatId` that no public API
  accepts — untagged, unbranded, and unresolvable by `view`, which is exactly
  the argument `get_comdat` makes for returning a handle instead. Comdat
  identity is `(module, ComdatId)` and is compared through `ComdatRef`'s
  `PartialEq`, so `a == b` replaces `a.id() == b.id()`.
- **Instruction metadata mutators take the `Unverified` module token.**
  `InstructionView::set_metadata`, `InstructionView::push_debug_record`, and
  their `Instruction` twins gain a leading `&Module<B, Unverified>` parameter.
  See *Fixed*, below, for what this closes.
- **The metadata currency is tagged and branded: `MetadataId<B>`.** Metadata was
  the one currency 2.0 left untagged — a handle was a bare `usize` arena index
  with neither a `ModuleId` tag nor a brand, so an *in-range* node minted in
  module A and attached in module B resolved against B's arena and printed the
  wrong node, silently. The split cycle A gave the value currency now reaches it:

  - **`MetadataSlot` is crate-internal**, together with `MetadataStore`, and is
    no longer re-exported from the crate root. It remains the bare arena index.
  - **`MetadataRef` is removed.** It was a `pub` newtype over a slot *with a
    public field*, so anyone could forge one. `MetadataId<B>` replaces it at
    every operand position: `m.metadata_tuple([node])` instead of
    `m.metadata_tuple([MetadataRef(node)])`.
  - **`MetadataId<B: ModuleBrand>`** is the public currency —
    `{ tag: ModuleId, slot: MetadataSlot }`, `Copy + Send + 'static`, brand
    phantom `PhantomData<fn(B) -> B>` like every other id. Two named brands make
    a cross-module mix-up a **compile error**; within one brand (two `DynBrand`
    modules, two generations of a re-issued brand) the tag makes it
    **`IrError::ForeignMetadataId`**, a new variant that is the metadata twin of
    `ForeignValueId`.
  - **Every metadata vocabulary type gained the brand:** `MetadataKind<B>`,
    `SpecializedMetadataNode<B>`, `MetadataField<B>`, `MetadataFieldValue<B>`,
    `DebugRecord<B>`, `DebugVariableRecord<B>`, `DebugMetadataOperand<B>`,
    `MetadataAttachmentSet<B>`, `NamedMDNode<B>`. `DebugMetadataOperand::Value`
    carries a `ValueId<B>` instead of a bare value slot, so
    `DebugMetadataOperand::Value(v.into_erased().id())` replaces
    `…::Value(v.into_erased().slot())`.
  - **Accepting an id makes an API fallible.** `Module::metadata_tuple`,
    `metadata_tuple_with_distinct`, `metadata_specialized`, `metadata_node`, and
    `metadata_as_value` now return `IrResult<…>`; `set_metadata` on
    `InstructionView` / `Instruction` / `FunctionValue` / `GlobalVariable` /
    `GlobalAlias` / `GlobalIFunc`, and `InstructionView::push_debug_record` (plus
    its `Instruction` twin), now return `IrResult<()>`. `metadata_string`,
    `metadata_constant`, and `metadata_reserve` stay infallible — they mint an id
    rather than consume one. `metadata_get` keeps returning `Option`, now `None`
    for a foreign id rather than another module's node.
  - **The read accessors hand back branded data.** `metadata()` on an
    instruction, function, or global returns an owned `MetadataAttachmentSet<B>`
    (was `Ref<'_, MetadataAttachmentSet>`), and `debug_records()` returns
    `Vec<DebugRecord<B>>` (was `Ref<'_, [DebugRecord]>`).
  - `llvmkit-asmparser`: `SlotMapping::metadata_nodes` is
    `NumberedValues<MetadataId<B>>`. The parser needed no raw-slot escape hatch —
    every id it hands back was minted by the module it is populating.

  **Printed IR is byte-identical**; the byte-locked example suites and the parser
  round-trip corpus are unchanged. Locked by
  `tests/module_ownership.rs::a_metadata_id_from_another_module_is_refused_everywhere`
  and `tests/compile_fail/cross_module_metadata_attachment.rs`.

#### Documentation

- The README's **Same-module safety** section, **D7**, and the three-run-modes
  example described the deleted generative lifetime brand and spelled
  `Module<'ctx, Brand<'ctx>, S>`. They now describe the three brand rungs
  (`module_new!` / `branded::<B>` / `dynamic`) and separate precisely what is
  compile-time (distinct brand types are a type error; the uniqueness registry
  is what makes a brand name one module) from what is run-time (modules sharing
  a brand type fall back to the `ModuleId` tag, surfacing as
  `IrError::ForeignValueId`, `None`, or a `view` panic).
- New README section, **Where llvmkit improves on upstream LLVM**: storable ids
  and an owned module (so a lifter can suspend, move threads, and resume),
  unrepresentable-versus-diagnosed error classes, and verification as a
  typestate rather than a function you must remember to call.
- New README section, **Bindings**: Python and Java bindings are planned and
  were blocked on exactly one thing — an API not yet stable enough to wrap,
  since wrapping a moving surface means rewriting the wrapper on every break.
  This release is what unblocks them. The section records the standing
  constraint 2.0 was designed under, which is why the surface is already
  wrappable: nothing reachable only from inside a closure, no lifetime in any
  storable type, `DynBrand` as the rung a wrapper uses, and misuse of a handle
  or id an `IrError` or a deterministic panic rather than a dangling read. A
  wrapper still supplies its own id table, since an id's `(ModuleId, slot)`
  payload is private and there is deliberately no `from_raw_parts`.
- `docs/type-safety-vs-llvm.md` worked examples re-spelled against the
  lifetime-free `Module<B, S>`.
- The **"2 environmental `.stderr` fixtures"** caveat is retired from
  `docs/design/pass-facing-type-safety.md`,
  `docs/design/unforgeable-markers-design.md`, and
  the `docs/future-work.md` backlog item that asked for a canonical re-bless.
  It was never real: both fixtures pass on the pinned 1.96.0 toolchain, and the
  mismatch only ever appeared under a newer rustc. Gated on `cargo +1.96.0` the
  trybuild baseline is **0 failures of 83 registered fixtures** (82
  `compile_fail` + 1 `pass`).

#### Added

- **`switch` and `invoke` can carry block arguments.** The block-argument
  authoring surface shipped `build_br_with_args` / `build_cond_br_with_args`
  and stopped there, so the public `IRBuilder` could *create* a `switch` or
  `invoke` edge into a parameterised block but had no way to supply its
  incoming values (the raw `add_incoming` is `pub(crate)`). Four new builders
  close it, all following the existing family's up-front, per-edge arity
  (`IrError::PhiArgArityMismatch`) and type (`IrError::TypeMismatch`) checks:
  - `build_switch_with_args(cond, default, cases, name)` and its width-erased
    twin `build_switch_dyn_with_args`, where `default` is a `(target, args)`
    pair and `cases` a `(case_value, target, args)` triple per edge. The whole
    case list is spelled at the call — an edge and the values it carries have to
    move together — so the returned `SwitchInst` comes back already
    `TermClosed`: there is no `add_case` on it, and therefore no way to bolt on
    a later case whose target's parameters nothing seeds.
  - `build_invoke_with_args(callee, args, normal, unwind, name)` and its
    erased-callee twin `build_invoke_dyn_with_args`, where `normal` and
    `unwind` are each a `(destination, args)` pair — both `invoke` edges are
    mandatory, so both are supplied; pass an empty slice for a destination with
    no parameters.

  All four bundle each edge with the values it carries into one parameter,
  which the case list forces anyway and which keeps `invoke`'s own call
  arguments and result name from crowding the signature. The frozen
  `build_br_with_args` / `build_cond_br_with_args` keep their flat
  `target, args` parameter pairs.
- Two compile-fail fixtures for 2.0 laws that had no lock.
  `builder_cannot_terminate_twice.rs` proves the *linearity* half of "one
  terminator per block" — every terminator-emitting build takes `self` by
  value, so a second call is `E0382`, where upstream `IRBuilder` keeps its
  insertion point after `CreateRetVoid()` and silently appends a second
  terminator. `view_cannot_outlive_its_module.rs` proves a borrowing handle
  cannot escape the scope of the owned module it was minted from (`E0597`) —
  the compile-time law that makes the id family necessary rather than merely
  convenient, since the `.id()` form of the identical program *does* compile.

#### Fixed

- **Instruction metadata now requires the `Unverified` module token.**
  `InstructionView::set_metadata` / `push_debug_record` (and their
  `Instruction` twins) took no token, while the metadata setters on
  `FunctionValue` and `GlobalVariable` — and `set_name` / `clear_name` on the
  very same type — always had. The omission punched through two guarantees at
  once: a `Module<B, Verified>`'s printed IR could be changed through a
  read-only `InstructionView` with the typestate still claiming verification
  (D8), and an `Inspect`-rung pass, which is handed only views, could rewrite
  `!dbg` attachments while the driver derived `Module<B, Verified>` and
  reported everything preserved. Both are now type errors, locked by
  `tests/compile_fail/verified_module_metadata_is_immutable.rs`.
- **`Module::metadata_set` and `Module::named_metadata_add_operand` are
  fallible.** The first silently no-opped on an unknown slot — the exact silent
  no-op the 2.0 contract forbids — and the second panicked with a bare
  `index out of bounds`. Both now return
  `Err(IrError::UnknownMetadataSlot { index, len })`.

#### Known gaps

**None left in the cross-module law.** This section carried one gap into the
freeze — metadata being the only currency 2.0 had not tagged, so an in-range
node from another module mis-resolved silently — and it was closed before 0.0.4
shipped. See "**The metadata currency is tagged and branded: `MetadataId<B>`**"
under *Changed (breaking)* above. Every public handle in the crate now states
its owning module both statically (the brand) and at run time (the `ModuleId`
tag), so a foreign handle is an `IrError` or a deterministic panic, never a
silent mis-resolve. Work still ahead is tracked in `docs/future-work.md`.

### The SSA session is a value (cycle D)

`SsaBuilder` converges on the cursor model. It is **one type** whose insertion
point is data, and the Braun bookkeeping moves into an owned, `Send`, `Clone`,
lifetime-free `SsaState<B>` that a caller stores in a struct field, snapshots
around a speculative branch, and drives one step at a time.

#### Changed (breaking)

- **`SsaBuilder<'m, 'ctx, B, F, S, R>` → `SsaBuilder<'s, 'ctx, B, F, R>`.** The
  `S: BuilderPositionState` parameter is gone: `switch_to_block` takes
  `&mut self` and returns `IrResult<()>` instead of changing the builder's type,
  and so does every terminator (`br` / `cond_br` / `switch` / `ret` /
  `ret_void` / `unreachable`; the last two became fallible). Operations that
  need an insertion point report the new **`IrError::SsaUnpositioned`** instead
  of not existing. This is the runtime rendering of a static law, taken *only*
  on the on-the-fly SSA layer, whose whole job is authoring a CFG discovered at
  run time — the plain `IRBuilder`'s linear block token and
  terminator-consuming cursor are untouched.
- **The session state is explicit.** Open it with `SsaState::for_function(&m,
  m.view(f))?` (this is where `SsaFunctionHasBlocks` is now raised), then mint
  working builders with `SsaBuilder::for_function(&m, m.view(f), &mut state)?`.
  Pairing a state with another function is `IrError::SsaForeignFunction`.
- **`ins()` and `current_block()` are fallible**: `b.ins()?.build_int_mul(..)?`.
  `ins()` still returns a **borrow** of the positioned plain builder, so its
  self-consuming terminators stay structurally unreachable through it.
- **The typed variable handles lost their lifetime**: `IntVariable<'ctx, W, B>`
  → `IntVariable<W, B>`, `FloatVariable<'ctx, K, B>` → `FloatVariable<K, B>`,
  `PointerVariable<'ctx, B>` → `PointerVariable<B>`. They are `Copy`,
  module-tagged ids like every other cycle-A/B id. Their `module()` accessor is
  removed with the `ModuleRef` they used to carry — the owning module is pinned
  by the brand type parameter.

#### Added

- **`SsaState<B>`** — owned, `Send`, `Clone`, no lifetime. `for_function`,
  `id`, `block_count`, `variable_count`.
- `SsaBuilder::is_positioned` / `clear_position` / `state`.
- `IrError::SsaUnpositioned`, `IrError::SsaForeignFunction`.
- **`Module::instruction_count()`** — total instructions across every block of
  every function. The module-size probe a transform driven to a fixpoint
  watches for a plateau.
- **`examples/lifter_session.rs`** — the consumer proof. A binary lifter as a
  plain struct owning its `Module`, an address→block `HashMap`, its `SsaState`
  and its cursor, driven by a suspend/resume `step()` loop, moved to a worker
  thread mid-function, and finished there. No closure, no borrow held across a
  step. Emits real, verified IR.
- **`examples/module_per_batch.rs`** — the JIT/batch shape: build a module,
  hand it away *by value* to a consumer, build the next one, all under one
  named brand the registry re-issues each round.
- `tests/block_id_stability.rs` — a `BlockId` minted before a block
  replace-all-uses still resolves *and still drives the mutation API*
  afterwards, so an address→block side map needs no hand-migration.

### Owned modules, branded by type (cycle C)

A module is now an ordinary owned value. `Module<'ctx, B, S>` becomes
`Module<B, S>`: it owns its storage, borrows nothing, and can be returned from a
function, stored in a struct field or a `Vec`, and moved across a thread
boundary. Identity moves from a generative lifetime to a `'static` **type**.

#### Removed (breaking)

- **`Module::with_new` and the lifetime brand `Brand<'id>` are gone.** Replace
  `Module::with_new("m", |m| { ... })` with `let m = module_new!("m")?;` and
  outdent — the body is otherwise unchanged, and the module now outlives it.
- **The `B: ModuleBrand = Brand<'ctx>` default type parameter is gone** from
  every handle, together with the defaults that preceded it in a declaration
  (`Term = Unterminated`, `Body = StructBodyDyn`, `E = ElemDyn`, `R = Dyn`,
  `P = TermOpen`, …), which Rust requires to be trailing. Spell the brand, or
  `_` where it is inferred: `IntValue<'_, i32, _>`, `Vec::<Type<_>>::new()`.
- **`Attribute::*_for_brand`** — the un-suffixed constructors are now the
  brand-generic ones. `Attribute::enum_attr` / `int` / `memory` / `string`.

#### Changed (breaking)

- **`ModuleBrand` requires `'static`.** It was already required by the brand
  registry, which keys by `TypeId`; the bound simply moves from the individual
  constructors onto the trait. A brand names a module, it never borrows one.
- **Brands are types.** `Module::branded::<B, _>(name)` claims a brand you name
  (at most one live module per brand, released on drop);
  `Module::branded_once::<B, _>(name)` retires it permanently on drop, so no
  successor can ever replay a stale `'static` id against fresh storage;
  `module_new!(name)` mints an unnameable brand per expansion site;
  `Module::dynamic(name)` is registry-exempt for a run-time module count.
  Collisions report `IrError::BrandInUse` / `BrandRetired`.
- **The schema traits take a `ModuleView`.** `IrField::ir_type`,
  `StructSchema::field_types` / `ir_type`, `FunctionReturn::ir_type`,
  `FunctionParam::ir_type` and `FunctionParamList::ir_types` now take
  `ModuleView<'ctx, B>` instead of `&'ctx Module<'ctx, B, Unverified>`; the
  `#[derive(IrStruct)]` `build` constructor follows. Call sites go from
  `X::ir_type(&m)` to `X::ir_type(m.as_view())`. Type construction is
  preservation-neutral, which is why the view already carried the constructor
  surface; `get_or_set_named_struct_body`, `named_struct` and
  `get_named_struct` join it. The typestate body setters (`set_struct_body`,
  `set_struct_body_dyn`) and every module-structural declaration stay on the
  `Module<Unverified>` token.

#### Added

- **Closure-free parser entry points**, re-exported at the crate root:
  `parse_into(module, src)`, `parse_branded::<B>(src)`, `parse_dynamic(src)`,
  `parse_file_branded::<B>(path)`, `parse_file_dynamic(path)` — each returns the
  owned `Module`, so it can be verified, stored, and moved. The closure forms
  remain for callers who need the `ParsedModule` slot mapping, which borrows the
  module it was parsed from and so cannot be returned alongside it.
  `ParseError` gains `BrandInUse` / `BrandRetired`.
- **`Module<B, S>` is `Send`** — including under a brand type that is itself
  `!Send`, because the brand rides as `PhantomData<fn(B) -> B>`. It stays
  `!Sync`: a module moves between threads, it is not shared between them.

#### Unchanged

Printed IR is byte-identical across the whole cycle — the byte-locked example
suites and the parser round-trip corpus pass untouched at every slice.

### Id-first handles (cycle B: builders speak ids)

Every builder and declaration now returns a storable id instead of a borrowing
handle. Combined with cycle A's id family, an IR value can be kept in a struct
field, a `HashMap`, or across a suspend/resume boundary — the thing the old
handle model could not express. Handles remain, as short-lived *views*.

#### Changed (breaking — this is the bulk of the cycle)

- **Builders return ids.** `build_*` yields `IntValueId<W>` / `FloatValueId<K>` /
  `PointerValueId` / erased `ValueId` per the result kind; `add_function*` yields
  `FunctionId<R>`; `add_global*` yields `GlobalId`. Reading from a result goes
  through a view: `b.view(x).ty()`, `m.view(f).param(0)`.
- **`IRBuilder::view`/`try_view` and `ModuleView::view`/`try_view`** join
  `Module::view`, so an id can be resolved wherever you are — mid-builder-chain
  or inside a pass.
- **Instruction ids.** Builders whose result carries its own API (`call`, the
  intrinsic call, `atomicrmw`, `cmpxchg`, `freeze`, `va_arg`, the phis) return an
  id whose view is that opcode handle, so the typed accessors survive:
  `b.view(call).return_int_value()`.
- **`BlockId` is the branch-target currency.** `BasicBlockLabel` is now only the
  view you get from `m.view(block_id)`; `.label()` is gone in favour of `.id()`.
  `BasicBlockEdge`, `BlockCall` and the SSA block wrapper lost their lifetime
  parameter as a result. `position_at_end_dyn(BlockId)` is the checked escape for
  dynamically discovered CFGs; `position_at_end` still takes the linear block
  token, so building into a terminated block stays impossible.
- **One `.id()` rule.** `.id()` mints the storable id; `.slot()` is the internal
  arena index. `IntrinsicInst::id()`'s legacy alias is now `intrinsic_id()`. A
  void instruction has no value id and so has no `.id()`.
- **The pass surface speaks ids** — analysis results and context accessors hand
  out ids for anything a pass would store, with views for reads. Rung tokens, the
  witnessed `done()`, and the erase-safe cursor are unchanged.
- **Operands accept ids everywhere.** Cast, comparison, pointer and callee
  positions that previously demanded a concrete handle now take the same
  `Into*`-style bounds as the rest, so a returned id feeds straight into the next
  builder with no rehydration. `IntoErasedValue` covers the positions that are
  erased by design; `IntoBasicBlockLabel` became module-taking and fallible so
  `BlockId` satisfies it. Erased `ValueId` is still rejected at typed positions —
  erasure must be spelled.
- The phi `Open`/`Closed` typestate is retired: a `Copy` id is re-mintable, so
  the marker could no longer express "exactly one open capability". A phi is
  still unobservable mid-construction from outside the crate (its constructors
  are crate-private), and the genuinely linear terminator states
  (`switch`/`indirectbr`/`landingpad`/`catchswitch`) are unchanged.

#### Added

- `PhiInst::remove_incoming` (and the other phi flavours) — previously absent.
  Mirrors upstream's swap-with-last backfill; it does not self-erase an emptied
  phi (llvmkit's erase consumes a linear handle, which a `Copy` opcode handle
  cannot express), and `Module::verify` reports the resulting predecessor
  mismatch.

#### Unchanged

Printed IR is byte-identical across the whole cycle — the byte-locked example
suites and the parser round-trip corpus pass untouched at every slice.

### Id-first handles (cycle A: foundations)

The first cycle of a redesign that replaces the closure-scoped, lifetime-branded
handle system with owned modules and storable, module-tagged id handles. This
cycle is additive and internal groundwork; later cycles flip the builders to
return ids and delete `Module::with_new`. The migration is spelled out break by
break in the `[0.0.4]` section above, under the cycle that made each one.

#### Added

- A public id family: `ValueId`, `IntValueId<W>`, `FloatValueId<K>`,
  `PointerValueId`, `FunctionId<R>`, `GlobalId`, `BlockId<R, _, Params>` — each a
  `Copy`, `Send`, module-tagged `{ module-tag, arena-slot }` pair that is stored
  and passed by value with no borrow of the module. Mint one from any value
  handle with `handle.to_id()`.
- `Module::view(id)` and `Module::try_view(id)` resolve an id back into its
  borrowing handle. `view` panics on an id from a different module (a caller
  contract violation, like an out-of-bounds index); `try_view` returns `None`.
  The module-tag check runs before any arena access.
- The typed ids implement the operand-conversion traits (`IntoIntValue` /
  `IntoFloatValue` / `IntoPointerValue`, and `IntoCallArg` for free), so they can
  be passed where a value operand is expected; a foreign-module id yields
  `Err(IrError::ForeignValueId)`. The erased `ValueId` deliberately does **not**
  implement them — erased → typed stays a spelled narrowing (`try_view`).
- `IrError::ForeignValueId` (non-breaking; the error enum is `#[non_exhaustive]`).

#### Changed (breaking)

- The internal arena-index types are renamed to `ValueSlot` / `TypeSlot` /
  `MetadataSlot` (freeing the `ValueId` / `TypeId` names for the public id
  family). `TypeId` and `MetadataId` are no longer re-exported under those names.
- `ModuleId` widened to a 64-bit counter; `ModuleId::as_u32` → `as_u64`.
- `Value::set_name` / `Value::clear_name` now panic when handed a module token
  from a different module, instead of silently doing nothing. All correct uses
  (which pass the value's own module token) are unaffected.

#### Removed

- The public `MetadataId::from_index` constructor (it had no callers and let
  external code forge a metadata id from an arbitrary index).

### Constant-folding parity with LLVM 22.1.4

An audit against the vendored `llvmorg-22.1.4` sources found the constant folder
faithful in the large, with a handful of divergences — one real bug, several
safe-but-not-identical over-precisions, and some missing folds. All are now
fixed so the folder mirrors upstream.

#### Fixed

- **Correctness:** `icmp` of a global vs `null` no longer folds in a non-zero
  address space (matching upstream's `!NullPointerIsDefined(AS)` guard, where
  `null` may be a valid address). It folded to a possibly-wrong constant before;
  address space 0 is unaffected.
- The instruction-level folder no longer threads the `exact` flag into a
  poison-producing path: `udiv/sdiv/lshr/ashr exact` with an inexact result now
  fold to the plain value (e.g. `udiv exact 7, 2 → 3`), and `x exact undef, 1`
  folds to `undef`, exactly as upstream's flag-agnostic `ConstantFoldInstruction`
  does — so llvmkit's two fold paths also agree with each other.
- `fcmp` equality with an `undef` operand folds to the concrete `i1`
  (`oeq undef,c → false`, `ueq undef,c → true`) rather than `undef` — the
  undef→undef shortcut is integer-`eq`/`ne`-only, matching `ICmpInst::isEquality`.
- A vector-condition `select` with an unresolvable lane falls through to the
  whole-value poison/undef rules instead of declining (so a poison arm still
  collapses to the other arm).

#### Added folds (previously declined; now matching upstream)

- FP **vector** arithmetic/compare through the DataLayout path (element-wise
  denormal flush), the `AllowNonDeterministic` fast-math fold guard, and
  `ptrtoint(inttoptr x)` sized by pointer width (`isEliminableCastPair` case 11).
- Pointer/int-cast `icmp` folds: `inttoptr` vs `null`, `ptrtoint` vs `0`,
  matching cast pairs, and same-base `(base+off1) pred (base+off2) → off1 pred
  off2`.
- `SymbolicallyEvaluateGEP` scalar offset canonicalization: an all-constant-index
  `getelementptr` folds to the `i8`-element form (`gep i32, @g, 4 → gep i8, @g,
  16`). Several upstream sub-cases (vector-index normalization, `in_range`,
  null/`inttoptr`-base, inbounds-inference for globals) are deliberately
  declined, never mis-folded.
- Same-base pointer `icmp` folds now compare the stripped bases by their
  underlying global rather than by arena identity — necessary because, unlike
  upstream's uniqued `Constant*`, llvmkit mints fresh ids for
  `GlobalValueRef`/`GepOffset` constants, so two independently-built pointers
  into the same global would otherwise fail to be recognized as same-base.

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
  the removed `module_mut` escape had. Taking one witnesses the mutator's
  dirty flag (handing out a mutable-positioned builder is intent-to-mutate),
  so a pass that builds through it cannot then `done()` an
  everything-preserved report and over-claim its analysis floor.
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
