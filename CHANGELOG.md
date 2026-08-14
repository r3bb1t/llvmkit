# Changelog

Notable, user-visible changes to `llvmkit`. The format follows
[Keep a Changelog](https://keepachangelog.com/); the project is pre-1.0, so
breaking changes are expected and are flagged inline. Until a tagged release is
cut, entries accumulate under **Unreleased**.

## [Unreleased]

> **These entries will also ship as 0.0.4.** crates.io has 0.0.3, so nothing
> below the `[0.0.4]` heading further down has shipped either — that section
> covers the id-first redesign (cycles A–E) specifically, and this one covers
> everything since. Two "unreleased" headings is an artifact of work continuing
> past the 0.0.4 freeze, not two pending releases; they collapse into one entry
> when the tag is cut.
>
> Every entry *below* the API idiomatics program predates its renames and keeps
> the spellings those cycles actually shipped (`IRBuilder`,
> `build_int_binop_erased`, `ZExtFlags`, ...). The program's bullets are the
> mapping to today's names; no earlier entry was rewritten to hide the change.

### Module-level entities

- **Changed: three metadata-operand diagnostics, and one new check.**
  `expected '{' here` from `parseMDNodeVector` (llvmkit said
  `expected metadata string or tuple`) — note it is lowercase where
  `parseNamedMetadata`'s own copy a few lines away is capitalised, and this is
  the path the fixture pins. `expected metadata operand` from
  `parseMetadata`'s fallthrough to `parseValueAsMetadata`, where llvmkit
  complained about a missing `!` instead of about the operand. And
  `invalid metadata-value-metadata roundtrip`, which llvmkit did not check at
  all: a `metadata`-typed operand would round-trip metadata through a value
  and back, and `!{metadata !0}` is the old syntax that hits it.

  With these, `invalid-mdnode-vector.ll`, `invalid-mdnode-vector2.ll`,
  `invalid-mdnode-badref.ll`, `invalid-metadata-has-type.ll` and
  `invalid-metadata-attachment-has-type.ll` all port verbatim.

- **Added: an `ifunc` carries the shared prefix clauses and takes metadata
  attachments.** `parseAliasOrIFunc` reads `dllstorage`, `thread_local` and
  `unnamed_addr` before it knows whether it has an alias or an ifunc, and
  applies all three to either — it simply never *prints* them for an ifunc,
  because `printIFunc` stops after visibility. `GlobalIfunc` stores them now.
  Separately, its property loop guards the metadata arm with
  `!IsAlias && MetadataVar`, so `@i = ifunc ..., !dbg !0` is legal and the
  alias spelling is `unknown alias or ifunc property!`; llvmkit accepted
  neither.

- **Fixed: `redefinition of global '@x'` was unreachable.** The variant and
  its rendering existed and were unit-tested, but the collision reached the
  *builder* first and surfaced as `expected valid global definition: a global
  named "g" already exists in this module`. The check now runs where upstream
  runs it, and skips a name that is present only as a forward reference —
  which that definition satisfies rather than collides with.

- **Fixed: `missing 'distinct', required for !DIAssignID()`.** llvmkit said
  `expected 'distinct', ...`. Its test called `parse_err` and discarded the
  result, so the message its doc comment named was never checked.

- **Added: `parseNamedMetadata`'s two special-cased operands.** A
  `!DIExpression(...)` may be written inline as a named-metadata operand and
  now parses; a `!DIArgList(...)` may not, and gets its own
  `found DIArgList outside of function`. llvmkit's loop accepted only `!N`
  slot references.

- **Fixed (over-strictness): an `ifunc` linkage is a verifier rule, not a
  parse rule.** `parseAliasOrIFunc` guards its `isValidLinkage` call with
  `if (IsAlias && ...)`, so upstream's parser checks *aliases* only and
  `Verifier::visitGlobalIFunc` carries the ifunc rule. llvmkit rejected it at
  parse time and again in `GlobalIfuncBuilder::build`, which is stricter than
  upstream — a divergence in its own right — and made upstream's own message
  unreachable. Both premature checks are gone; the new
  `VerifierRule::IfuncInvalidLinkage` carries the text verbatim, bang
  included. The alias half stays a parse error, as upstream has it.

- **Added: `invalid type for global variable`**, which llvmkit did not check
  at all. Two halves: a function value type, and
  `PointerType::isValidElementType` — `label`, `metadata`, `token`, `x86_amx`
  — since a global's value type is the pointee of its own `ptr`.

- **Changed: a declaration-linkage global takes no initializer, rather than
  rejecting one.** `if (!HasLinkage || !isValidDeclarationLinkage(Linkage))`
  guards upstream's `parseGlobalValue` call and there is no lookahead behind
  it, so `@g = external global i32 0` leaves the `0` unconsumed and it fails
  at top level. llvmkit peeked ahead and reported an invented
  `no initializer: a global with 'external' linkage is a declaration` — the
  same rejection reached by guessing instead of by the rule. The lookahead is
  deleted.

- **Changed: `void type only allowed for function results`.** llvmkit said
  `expected non-void type (void only allowed at function results)`, under a
  test whose doc comment named upstream's wording and called the difference
  "a structured error". It was neither structured nor upstream's.

- **Fixed: local linkage did not constrain visibility on globals or
  functions.** `@var = internal hidden global i32 0` and
  `define internal hidden void @f()` were both accepted;
  `isValidVisibilityForLinkage` and `isValidDLLStorageClassForLinkage` are
  asked at three call sites upstream, and llvmkit had them on the *alias* path
  only. The predicate is now one function the three sites share, and the whole
  twelve-fixture family
  `{internal,private}-{hidden,protected}-{alias,function,variable}.ll` is
  ported.

- **Changed: the property diagnostics carry upstream's text.**
  `unknown target property`, `unknown global variable property!` and
  `unknown alias or ifunc property!` — both bangs included — plus
  `An alias or ifunc must have pointer type`,
  `Metadata id is already used` (capitalised, like `parseScope`'s and
  `parseOrdering`'s), and `unexpected type in metadata definition`, which
  llvmkit had no check for at all: upstream detects `!0 = metadata !{}`, the
  old syntax, and says so rather than reporting a generic failure. Two of
  these had been routed through the `Expected` variant, rendering
  `expected unknown alias or ifunc property` — the word glued onto a message
  that is not one.

### Global variables

- **Added: `code_model "small"` and the four sanitizer keywords on globals**
  (`no_sanitize_address`, `no_sanitize_hwaddress`, `sanitize_memtag`,
  `sanitize_address_dyninit`) — both parsed and printed, so
  `test/Assembler/globalvariable-attributes.ll`'s `@g5`–`@g14` round-trip.
  `llvmkit_ir::CodeModel` mirrors `CodeModel::Model` and
  `SanitizerMetadata` mirrors `GlobalValue::SanitizerMetadata` as one
  `Option` on the global rather than upstream's presence-bit-plus-side-table,
  which is a C++ allocation artifact. The sanitizer keywords **accumulate**:
  `parseSanitizer` merges into what the global already carries, and the
  printer emits them in its own fixed order, before `comdat` and `align`.

### Calling conventions

- **Fixed: `expected metadata after comma`.** llvmkit said
  `expected metadata attachment` at `parseInstructionMetadata`'s only
  diagnostic. Found by porting
  `test/Assembler/alloca-addrspace-parse-error-{0,1}.ll`, which also pin
  something easy to miss: a misordered `alloca i32, addrspace(1), align 4`
  reports through *that* message, not a dedicated one, because the loop
  demands metadata once a comma has been eaten.

- **Changed: the optional-parser diagnostics carry upstream's text.**
  `parseScope`'s three messages open with a capital `E` —
  `Expected '(' in syncscope`, `Expected synchronization scope name`,
  `Expected ')' in syncscope` — alone among LLParser's diagnostics, and so does
  `Expected ordering on atomic instruction`. Reproduced as written; the
  capital is contractual, not a typo to tidy. `parseOrdering` has that one
  message at every call site, so llvmkit's six per-site labels (`expected
  fence ordering`, `expected cmpxchg success ordering`, …) are gone — the
  per-instruction complaints upstream *does* have are validity checks that run
  after it, not alternative spellings of it. Also
  `expected localdynamic, initialexec or localexec`,
  `expected ')' after thread local model` (unhyphenated, as upstream writes
  it), and `unknown selection kind` for a bad comdat.

- **Fixed: 28 calling conventions parsed as `ccc`.** The parser matched 31
  keywords while the printer knew 60 — so `spir_kernel`, `ptx_kernel`,
  `graalcc`, `preserve_nonecc`, `cxx_fast_tlscc`, `x86_intrcc`, the ARM/AArch64
  sets (including all three `aarch64_sme_preservemost_from_x*`), the AMDGPU
  additions (`amdgpu_gfx`, the two `cs_chain` forms, `gfx_whole_wave`), the
  AVR/MSP430 interrupt conventions and the three CHERIoT ones were all read as
  the default and printed back wrong. Every keyword upstream's
  `parseOptionalCallingConv` accepts is now accepted, `riscv_vls_cc(<N>)`
  included, with `unknown RISC-V ABI VLEN` for a width outside the twelve legal
  ones.

  The lexer already had all 59 keywords and the printer all 60 mnemonics; only
  the parser's table was short. A new drift lock walks the whole id space and
  asserts that everything llvmkit can *print* it can *read back*, which is the
  invariant that was silently false.

- **Changed: `cc <N>` accepts any `u32`.** `CallingConv::from_raw` is now
  infallible. It used to reject anything above `MaxID` (1023), but that
  constant bounds the bitcode field — `parseOptionalCallingConv`'s `cc` arm is
  a bare `parseUInt32(CC)` and the Verifier never mentions it — so `cc 1024`
  was a parse error llvmkit invented.

### Attributes

- **Added: `initializes((-4, 0), (4, 8))`, and with it `ConstantRangeList`.**
  The last attribute in `Attributes.td` that llvmkit did not model. The new
  `llvmkit_ir::ConstantRangeList` ports
  `llvm/include/llvm/IR/ConstantRangeList.h` — the ordering invariant
  (`isOrderedRanges`), the checked constructor (`getConstantRangeList`), the
  merging `insert`, and `print`. Note that the invariant rejects ranges that
  merely *touch*: `initializes((0, 4), (4, 8))` is an error, because the
  ordering test is `sle`. All eight of `LLParser::parseInitializesAttr`'s
  diagnostics are reachable, including
  `Invalid (unordered or overlapping) range list`. With this, **every
  attribute LLVM 22.1.4 declares is accepted in every position it declares**
  — the drift guard's `NOT_YET_MODELED` list is empty.

- **Changed: `parseUInt32` / `parseUInt64` speak upstream's texts.** Every one
  of upstream's ~25 call sites reports the same `expected integer`; llvmkit
  passed a bespoke label per site (`expected alignment (bytes)`,
  `expected uselistorder index`, …), so all 25 diverged. `parseUInt32`'s
  second message, `expected 32-bit integer (too large)`, existed nowhere — it
  had been collapsed into the first by parsing straight into a `u32`. That
  distinction is load-bearing: it is why `attributes #0 = { align = 4294967296 }`
  is rejected while the inline `align 4294967296` is accepted.

  One site was wrong to share the helper at all: `parseDIExpressionBody`
  inspects the token itself and says `expected unsigned integer`. It now does
  too.

- **Fixed (P0): an argument-carrying attribute on a function header did not
  parse at all.** `define void @f() uwtable { ret void }` — ordinary `clang`
  output — failed with `expected '{' to open function body`, and so did
  `allocsize`, `vscale_range`, `allockind`, `nofpclass`, `dereferenceable`,
  `captures`, `range`, `initializes` and the six type attributes. llvmkit
  gates the header attribute list behind a lookahead predicate that had gone
  out of sync with the list's own arms; upstream enters the list
  unconditionally. Every test for these attributes had used the
  `attributes #N = { … }` form, which enters the loop directly, so the header
  path was untested.

- **Added: the attribute-group `align = N` / `alignstack = N` grammar**, and
  with it the `InAttrGrp` printing form. `Attribute::getAsString` spells four
  kinds differently inside a group — `align=8`, `alignstack=8`,
  `dereferenceable=8`, `dereferenceable_or_null=8` — and
  `parseEnumAttribute` accepts only the matching grammar in each context, so
  `attributes #0 = { align 8 }` is now the error upstream gives
  (`expected '=' here`) and the group prints `align=8`.
  `test/Bitcode/attributes.ll` pins both halves: it writes `alignstack(4)`
  inline and checks `alignstack=4` in the group.

- **Added: the alignment and dereferenceable value checks.**
  `alignment is not a power of two`, `huge alignments are not supported yet`
  (the maximum is `1 << 32`, inclusive), `stack alignment is not a power of
  two`, and `dereferenceable bytes must be non-zero` — none of which existed.
  `@g = global i8 0, align 8589934592` used to be accepted silently, and
  `align 3` reported an invented `expected alignment must be non-zero power of
  two, got 3`. Also `expected unwind table kind`,
  `unterminated attribute group`, and
  `cannot have an attribute group reference in an attribute group`, plus the
  `attributes` group's own `expected '=' here` / `expected '{' here` /
  `expected end of attribute group` texts and the anchor fix that puts
  `attribute group has no attributes` on the `attributes` keyword.

- **Fixed: legacy memory keywords now intersect instead of accumulating.**
  `upgradeMemoryAttr` runs `ME &= MemoryEffects::X()` over one accumulator
  that starts at `unknown()` and is emitted once after the whole attribute
  list. llvmkit added one `memory(...)` attribute per keyword, so
  `declare void @f() readonly writeonly` printed
  `memory(read) memory(write)` — two attributes of a kind LLVM can only hold
  one of — where upstream gives `memory(none)`. `MemoryEffects` gains
  `BitAnd` / `BitAndAssign` (ports of `MemoryEffectsBase::operator&`), and
  `AttributeStorage::set` ports `addAttributeImpl`'s replace-by-kind branch,
  which is also what makes a legacy keyword discard an explicit `memory(...)`
  from the same list in either source order — `memory(none) readonly` and
  `readonly memory(none)` are both `memory(read)`.

- **Changed: `range(T lo, hi)` reaches 1:1 with `parseRangeAttr`.** All seven
  of its diagnostics now carry upstream's exact text —
  `the range must have integer type!` (anchored at the *first* token of the
  type, and false for a vector of integers),
  `the range represent the empty set but limits aren't 0!` (checked before
  the closing `)`, reproducing upstream's own ungrammatical wording), and the
  bare `expected '('` / `','` / `')'` / `expected integer` forms in place of
  llvmkit's embellished ones. Three inputs change verdict with the token-width
  model below: `range(i8 -255, 0)` and an all-zero
  `range(i8 u0x000000000000000000, 1)` were silently accepted (the first
  wrapping its bound to 1) and are now rejected as too large; `range(i8 1, 0)`
  — a *wrapped* range — parses cleanly, as `test/Verifier/range-attr.ll`
  requires.

- **Changed (behavioural): an integer literal now carries the width the token
  needs, not the width its context wants.** Two consequences, both upstream's:
  `s0x0F` is **−1**, because `LLLexer` truncates a `[us]0x` literal to its
  active bits *before* the prefix decides the signedness (llvmkit read `+15`,
  a wrong value rather than a missing error); and `i8 300` is now accepted as
  `44`, because `convertValIDToValue` applies `extOrTrunc` and not a checked
  widening (llvmkit refused it as an overflow). The token width is also what
  makes `range`'s
  `integer is too large for the bit width of specified type` askable at all,
  so that diagnostic lands here rather than with the rest of `range`.

- **Added: `captures(...)` in full.** llvmkit accepted only `captures(none)`
  and mapped it to `nocapture`; every other component was a hand-written
  refusal. `CaptureComponents` and `CaptureInfo` port
  `llvm/Support/ModRef.h`, including the fact that these are a **lattice, not
  four flags** — `ADDRESS` literally contains `ADDRESS_IS_NULL`'s bit, so
  "captures the address but not its nullness" is unrepresentable rather than
  merely invalid. The `ret:` sublocation parses at any position, once, and
  swallows every later component; a missing `ret:` means the two buckets are
  *equal*, not that the return captures nothing. `captures(none)` now prints
  as itself instead of `nocapture`.

- **Added: an attribute written in the wrong position is now rejected.**
  `AttrKind::positions` ports the `[FnAttr]` / `[ParamAttr]` / `[RetAttr]`
  lists from `Attributes.td`, and `can_use_as_fn_attr` and its two siblings
  read it, so `this attribute does not apply to functions` / `to parameters` /
  `to return values` all fire where upstream fires them. `align` keeps the
  exemption upstream calls out by name — the function loop accepts it despite
  the `.td`, because it is later moved to the alignment field.

- **Added: `allocsize`, `vscale_range` and `allockind`**, the three attributes
  whose argument needed a grammar. With them come upstream's diagnostics —
  `'allocsize' indices can't refer to the same parameter`,
  `unknown allockind <word>`, and `expected allockind value` — and a new
  `AllocFnKind` flag type mirroring `llvm/IR/Attributes.h`.

  Upstream packs each of these into a single `uint64_t` and reserves a
  sentinel in it for "absent" (`-1` for `allocsize`'s element count, `0` for
  `vscale_range`'s maximum). llvmkit stores them as named fields with an
  `Option`, so the sentinel exists only where it is printed. Note that
  `vscale_range(4)` means `vscale_range(4,4)`, not an unbounded maximum —
  upstream defaults a missing maximum to the minimum.

- **Added: `preallocated(T)`**, the one type attribute `Attributes.td`
  declares in both function and parameter position. It takes the same
  production as `byval` / `sret`.

- **Fixed (`llvmkit-ir`): printing an attribute *group* could panic.** The
  group printer used the module-less `Display`, whose type-attribute arm is
  `unreachable!("typed attributes need a module context to print")`. Nothing
  reached it while no type attribute was accepted in function position;
  `preallocated` in an `attributes #0 = { … }` group does. The group printer
  now takes the module-aware path the parameter printer already used.

- **Added: thirty-nine attributes the parser did not accept.** Every remaining
  plain `EnumAttr` in `Attributes.td` — `naked`, `nobuiltin`, `returns_twice`,
  the eight `sanitize_*` kinds, the coroutine and hot-patch flags,
  `swifterror`, `noext`, `dead_on_return`, and the rest. The lexer already
  tokenised all of them and `llvmkit-ir` already had most of the kinds; the
  gap was `attr_kind_for_keyword`, upstream's `tokenToAttribute`, so each one
  parsed as "not an attribute" and quietly ended whatever list it appeared in.
  Fourteen new `AttrKind` variants come with them, named after their upstream
  `Attributes.td` defs.

- **Fixed (test): the attribute drift guard could not read its own source.**
  It exists so a new upstream attribute fails CI, and its reader was wrong
  twice over: it worked line by line, so it missed every `def` upstream wraps
  across lines, and it required `def Name : Kind<…>` with spaces around the
  colon, so it also missed the three written `def Name: Kind<…>`. Six
  attributes were affected — three invisible, three probed in no position at
  all, which passes vacuously. Fixing both surfaced four real gaps:
  `speculative_load_hardening`, `disable_sanitizer_instrumentation`,
  `allocalign` (which needed a new `AttrKind`), and `allockind`, which needs a
  grammar and is listed as such.

### Constants must agree with the type asked for

- **Breaking (parser): `parse_constant_value` accepts only the `ValID` kinds
  upstream's `parseConstantValue` accepts.** It used to convert whatever
  `parseValID` returned, which meant it took `@g` and `[]` — neither is in
  upstream's set. Both now report `expected a constant value`, previously
  unreachable. `null` keeps its special handling: upstream takes
  `Constant::getNullValue(Ty)` directly, so at a non-pointer type it is that
  type's zero rather than `null must be a pointer type`.

- **Breaking (parser): floating-point literals go through `double`, as
  upstream's lexer does.** `LLLexer` has no type information, so it reads every
  decimal literal at `IEEEdouble` and `convertValIDToValue` narrows afterwards;
  llvmkit read the literal straight at the demanded type's semantics. Three
  consequences, all upstream's: a decimal at `half` or `bfloat` is legal only
  when it survives the double round-trip exactly (`floating point constant
  invalid for type` otherwise, where llvmkit used to round silently to a
  *different* value); `fp128`, `x86_fp80` and `ppc_fp128` have no decimal
  spelling at all and now say so with `floating point constant does not have
  type 'T'`, previously unreachable; and a signalling NaN survives the
  narrowing, which quiets it, by being rebuilt afterwards.

- **Added (`llvmkit-ir`): `float_value_is_valid_for_type`**, a port of
  `ConstantFP::isValueValidForType`.

- **Fixed (parser): aggregate constants no longer parse *from* the type they
  are checked against.** `[...]`, `<...>`, `<{...}>` and `{...}` were selected
  by the demanded type and built directly at it, where upstream builds them
  from their own elements and lets `convertValIDToValue` compare. Two checks
  were unreachable as a result and are new:
  `packed'ness of initializer and type don't match`, and the bare
  `constant expression type mismatch` for a struct initializer at a non-struct
  type. `[]` at a zero-length array now materialises `poison`, matching
  upstream's `t_EmptyArray` arm, rather than a zero-length array constant.

- **Fixed (parser): `c"..."` is always an `[N x i8]` array.** llvmkit derived
  the array type from the *expected* type, so `@g = global [4 x i32] c"abcd"`
  was silently accepted and built a `[4 x i32]`. Upstream's
  `ConstantDataArray::getString` always builds `[N x i8]` and leaves agreement
  to `convertValIDToValue`, which now reports
  `constant expression type mismatch: got type '[4 x i8]' but expected
  '[4 x i32]'`.

- **Fixed (parser): the two `splat` conversion diagnostics.**
  `vector constant must have vector type` and
  `constant expression type mismatch: got type 'A' but expected 'B'` replace a
  single `expected vector type for splat constant` covering both.

- **Fixed (parser): `expected value token`** replaces `expected constant
  initializer` in `LLParser::parseValID`'s default arm, and
  `expected a function, alias to function, or ifunc in dso_local_equivalent`
  is new — the rule existed only as a builder error wrapped in
  `expected dso_local_equivalent: ...`.

- **Added (`llvmkit-ir`): inline-asm constraint strings are parsed and
  verified.** `parse_constraints` and `verify_inline_asm` port
  `InlineAsm::ParseConstraints` and the static `InlineAsm::verify`, with
  `ConstraintInfo` / `SubConstraint` / `ConstraintKind` as the model. All nine
  of `verify`'s messages are now reachable from the parser, byte for byte;
  the constraint string used to be stored and never looked at. This closes the
  recorded future-work item "inline-asm constraints are never parsed".

- **Fixed (parser): three inline-asm texts, and inline asm as a value.**
  `expected string constant`, `expected comma in inline asm expression` and
  `expected constraint string` replace llvmkit wordings; `asm` outside a call
  callee reports `invalid type for inline asm constraint string` instead of
  `expected supported constant/value form`.

- **Fixed (parser): `%res = callbr ...` parses.** `callbr` was dispatched only
  when it opened the line, so a `callbr` binding a result was rejected as an
  unsupported opcode.

- **Fixed (parser + `llvmkit-ir`): the two inline-asm label rules moved to the
  verifier**, where `Verifier::verifyInlineAsmCall` puts them. The parser
  rejected both with invented wordings, shadowing the ordinary-call rule
  llvmkit's verifier already carried with upstream's exact text; the `callbr`
  twin, `Number of label constraints does not match number of callbr dests`,
  is new.

- **Fixed (`llvmkit-ir`): `label_constraint_count` counts parsed constraints**
  rather than occurrences of `!`, which also matched a `!` inside a `{...}`
  register name.

- **Fixed (parser): `getelementptr` constant expressions run upstream's
  checks, in upstream's order.** `invalid getelementptr indices` is new —
  llvmkit's constant-expression path never asked
  `GetElementPtrInst::getIndexedType`, though its instruction path and
  verifier both did. `base element of getelementptr must be sized` and
  `invalid base element for constant getelementptr` were checked in the wrong
  order and, for the scalable case, at the wrong moment (right after the
  source type parsed, before the operand list). Order is behaviour here: a
  struct holding a scalable vector satisfies neither check, and upstream
  reports it unsized.

- **Fixed (`llvmkit-ir`): `Type::is_sized` follows `StructType::isSized`.**
  A struct holding a scalable vector is now unsized unless its body is
  homogeneously that vector (`containsHomogeneousScalableVectorTypes`), and a
  target extension type defers to its layout type instead of answering `true`
  unconditionally.

- **Added (`llvmkit-ir`): `Type::is_scalable` and `indexed_gep_type`**, ports
  of `Type::isScalableTy` (including `isScalableTargetExtTy`) and
  `GetElementPtrInst::getIndexedType`. Three private near-copies of the
  scalable-vector walk — in `constants.rs`, the constant folder and the
  parser — collapse onto the first; each was missing at least one of the
  target-extension arm, the array recursion, or the cycle guard.

- **Fixed (`llvmkit-ir`): a struct index may be a vector splat.**
  `StructType::indexValid` accepts a `<N x i32>` index whose lanes agree and
  reads it through `getSplatValue`; llvmkit required a scalar `i32`, which
  rejected `getelementptr({ i8 }, <2 x ptr> undef, <2 x i64> …, <2 x i32>
  zeroinitializer)` — upstream's own `ConstantExprFold.ll`.

- **Fixed (parser): `parseGlobalValueVector`'s two early returns.** A closing
  bracket now yields an empty operand list rather than a diagnostic from the
  first element parse, and `inrange` ends the list for the caller to handle.

- **Breaking (`llvmkit-ir`): `IrError::RecursiveStructBody { name }`** replaces
  `InvalidOperation { message: "recursive struct body" }`, and renders
  `StructType::checkBody`'s own text — `identified structure type '<name>' is
  recursive` — which `parseStructDefinition` prints verbatim.

- **Fixed (parser): eight aggregate-initializer diagnostics.**
  `constant vector must not be empty` (llvmkit accepted `<>`),
  `vector elements must have integer, pointer or floating point type`,
  the numbered element-agreement messages `vector element #N is not of type
  'T` / `array element #N ...` — reproduced with upstream's own unbalanced
  quote, since diagnostic text is contractual — `invalid array element type`,
  `invalid empty array initializer` (`[]` is legal only at a zero-length
  array type), `initializer with struct type has wrong # elements`, and
  `element N of struct initializer doesn't match struct element type`. All
  previously surfaced as llvmkit builder errors wrapped in
  `expected valid <kind> constant: …`, or not at all.

- **Fixed (parser): six `convertValIDToValue` diagnostics.**
  `functions are not values, refer to them as pointers` (the guard before any
  `ValID` arm runs), `invalid use of function-local name`, `integer constant
  must have integer type`, `floating point constant invalid for type`,
  `null must be a pointer type`, and the first-class guard the `undef` /
  `poison` / `zeroinitializer` arms share — which llvmkit had no equivalent
  of, so `undef` at an opaque struct type was accepted. The rest were llvmkit
  wordings rendered as `expected <production>`.

- **Fixed (`llvmkit-ir` + parser): `invalid cast opcode for cast from 'A' to
  'B'`.** `CastInst::castIsValid` is ported onto `cast_is_valid`, and the
  constant-expression cast path asks it. llvmkit asked a different question
  entirely — whether the cast's destination matched the *initializer's* type —
  which upstream never asks; that agreement belongs to `convertValIDToValue`
  and is checked there. `test/Assembler/invalid_cast4.ll` is ported.

- **Added (`llvmkit-ir`): seven `Type` predicates from `Type.h`** —
  `scalar_type`, `scalar_size_in_bits`, `primitive_size_in_bits`,
  `vector_element_count`, `is_int_or_int_vector`, `is_ptr_or_ptr_vector`,
  `pointer_address_space`. Several existed as private duplicates in five
  different modules; these are the public ones `cast_is_valid` and the
  verifier can share.

Fifth wave of the `LLParser` parity program.

- **Fixed (parser): `constant expression type mismatch: got type 'A' but
  expected 'B'`.** A parsed constant carries its own type and nothing checked
  it against the context's — the `ValID::t_Constant` arm of
  `convertValIDToValue`. `blockaddress` is the usual way to reach it, since
  its type comes from the *function's* address space rather than the
  surrounding expression, which is exactly what
  `test/Bitcode/blockaddress-addrspace.ll`'s negative half tests. Both halves
  of that fixture now behave as upstream.

### Type identity: `%0 = type {i32}` and `%1 = type {i32}` are two types

Fourth wave of the `LLParser` parity program. LLVM has two struct-identity
regimes — *literal* structs are structurally uniqued, *identified* ones never
unify — and llvmkit spelled the difference as `name.is_none()`. That cannot
represent an **anonymous identified** struct, which is exactly what a numbered
type definition is.

- **Breaking (`llvmkit-ir`): struct identity is explicit.** `StructTypeData`
  carries a `StructIdentity` (`Literal` / `Identified { name }`) instead of an
  `Option<String>` name, and `Module::anonymous_identified_struct()` mirrors
  `StructType::create(Context)` with no name. The printer numbers an anonymous
  identified struct by its position among the module's anonymous ones, as
  `TypePrinting::NumberedTypes` does.

- **Fixed (`llvmkit-ir` + parser): element and shape validity.** The six
  `isValidElementType` / `isValidReturnType` / `isValidArgumentType`
  predicates from `llvm/lib/IR/Type.cpp` are ported onto `Type`, deny-lists
  reproduced as deny-lists so a type kind added later keeps upstream's default
  answer — and the vector one as the allow-list it actually is, target
  extension types' `CanBeVectorElement` included. The parser checks them where
  upstream checks them, which makes five diagnostics reachable:
  `zero element vector is illegal`, `size too large for vector`,
  `invalid vector element type`, `invalid array element type`, and
  `invalid element type for struct` — the last per element, against that
  element's own location. `<0 x i32>`, `<2 x {i32}>` and `[2 x label]` were
  all accepted before.

- **Fixed (parser): function-type validity.** `invalid function return type`
  and `invalid type for function argument` now fire, and so do
  `argument name invalid in function type` /
  `argument attributes invalid in function type` — which exist only because
  upstream shares `parseArgumentList` between a function *type* and a function
  *header*, so a name and attributes parse in type position and are rejected
  afterwards. llvmkit read bare types there, leaving both behind a generic
  `expected ')'`.

- **Fixed (parser): symbolic and defaulted address spaces.**
  `addrspace("A")` / `("G")` / `("P")` now resolve through the module's data
  layout to the alloca, default-globals and program address spaces
  (`parseOptionalAddrSpace`'s `ParseAddrspaceValue`), with
  `invalid symbolic addrspace 'X'`, `expected integer or string constant`, and
  the `isUInt<24>` check `invalid address space, must be a 24-bit integer`.
  And a function that declares no `addrspace` now takes the **program**
  address space rather than 0 — upstream's `DefaultAS` parameter, reached
  through `parseOptionalProgramAddrSpace`, which llvmkit had no equivalent of.

  Together with W2.6's same-function forward `blockaddress`, this clears
  `test/Bitcode/blockaddress-addrspace.ll::return-self-good.ll`, whose corpus
  entry moves `xfail-parse` -> `pass`.

- **Fixed (`llvmkit-ir` + parser): target extension type validation.**
  `TargetExtType::checkParams` is ported — the three named types that
  constrain their own arity (`aarch64.svcount`, `riscv.vector.tuple`,
  `amdgcn.named.barrier`) — and the parser surfaces it. A type parameter
  after an integer one is now `expected uint32 param` rather than a generic
  complaint, and a type parameter may be `void`, which upstream allows
  (`AllowVoid=true`) and llvmkit rejected.

- **Fixed (parser): the legacy typed-pointer path parses its pointee.**
  `parse_type` opened with a lookahead that skipped a `<type> '*'` shape
  syntactically and lowered it straight to opaque `ptr`, so the pointee type
  was never built. Every check about it was therefore dead code, and it was
  also why `%t*` never looked `%t` up. `parse_type` now follows
  `LLParser::parseType`'s own shape — atom, then a suffix loop — which makes
  `basic block pointers are invalid`, both spellings of `pointers to void are
  invalid`, and `pointer to this type is invalid` reachable, and lets an
  undefined `%t` in `%t*` be reported. Nine now-dead syntax-skipping helpers
  (186 lines) are deleted.

- **Fixed (parser): `ptr*` is rejected.** It parsed as plain `ptr`. The check
  now sits where upstream puts it, in the opaque-pointer arm, which only
  falls through to the suffix loop when a function-type `(` follows.

- **Fixed (parser): numbered types have identity.** `%N = type ...` minted a
  fresh *literal* struct, so two numbered types with equal bodies silently
  became one type, and a forward-referenced `%N` could never be the same type
  as its later definition. Both now work.

- **Fixed (parser): a numbered type may be defined at any slot.** llvmkit
  required each definition to equal a running frontier and rejected
  `%5 = type {i32}` as the first one; upstream's `NumberedTypes` is a plain map
  with no such rule.

- **Fixed (parser): `redefinition of type`, `forward references to non-struct
  type`, `use of undefined type '%N'` and `use of undefined type named 'x'`.**
  None were reachable. The type tables kept no forward-reference location — the
  one bit that drives all four — and a comment asserted that upstream does not
  diagnose an undefined type at all, which is not so: `validateEndOfModule`
  has a loop for each spelling. `%t = type opaque` now correctly counts as a
  definition, so a later `%t = type {i32}` is a redefinition.

### Forward references: use-before-definition, as upstream resolves it

Third wave of the `LLParser` parity program. Upstream mints a typed sentinel
the first time a name is used and replaces it when the definition arrives
(`LLParser::getGlobalVal`, `PerFunctionState::getVal` / `setInstName`).
llvmkit reported `use of undefined value` immediately, rescued only by a
handful of construct-specific deferral lists. This wave builds the general
mechanism and retires those lists.

- **Breaking (`llvmkit-ir`): the forward-`blockaddress` placeholder is now the
  general forward-reference placeholder.** `BlockAddressPlaceholder` is
  renamed `ForwardRefValue` and `Module::block_address_placeholder` is renamed
  `Module::forward_ref_value_placeholder`; the payload accepts any first-class
  type, not only a pointer, because a function-local sentinel carries whatever
  type its first use demanded — upstream's `new Argument(Ty)`. The handle
  stays linear: a sentinel is resolved exactly once, so
  `replace_all_uses_with` consumes it.

- **Breaking (`llvmkit-ir`): `ForwardRefValue::replace_all_uses_with` takes a
  `Value`, not a `Constant`.** A forward-referenced name is usually defined by
  an instruction, which is what upstream's `Sentinel->replaceAllUsesWith(Inst)`
  passes. The underlying walker is correspondingly category-agnostic;
  replacing a value that a *uniqued constant* embeds still requires a constant
  replacement, and now says so instead of interning a constant with a
  non-constant operand.

- **Fixed (`llvmkit-ir`): a global object's single-slot fields are uses.**
  An initializer, an aliasee, an ifunc resolver and a function's
  personality / prefix / prologue are ordinary `Use` edges upstream, because
  `GlobalValue` is a `User`. llvmkit stored each in a bare `Cell` with no
  reverse edge, so `num_uses` undercounted them and RAUW could not reach
  them — `@a = global ptr @b` would keep pointing at whatever `@b` used to
  be. Each of the six now registers a `GlobalField` edge, and both RAUW
  walkers rewrite the cell.

- **Fixed (parser): a function-local value may be used before it is
  defined.** `%a = add i32 %b, 1` followed by `%b = add i32 2, 3` is ordinary
  `.ll`; llvmkit answered `use of undefined value` at the first line. Every
  local operand now goes through a port of
  `LLParser::PerFunctionState::getVal`, and every instruction result through a
  port of `setInstName`. The four construct-specific deferral lists that used
  to paper over particular cases — phi incomings, `atomicrmw` values — are
  deleted, along with the `undef` stand-in they parked in the operand
  meanwhile.

- **Fixed (parser): a `@` symbol may be referenced before it is defined.**
  `@a = global ptr @b` with `@b` below it is upstream's own
  `test/Assembler/2009-02-01-UnnamedForwardRef.ll`; llvmkit answered
  `use of undefined global`. A reference to an unknown `@` name or slot now
  mints a stand-in at the demanded pointer type
  (`LLParser::getGlobalVal`), and `validateEndOfModule` retires it against
  whatever definition arrived. A reference nothing satisfies is
  `use of undefined value '@x'` — upstream's noun for an unsatisfied *use*
  is `value`, where a colliding *definition* stays `global`; `SymbolKind`
  gained a variant rather than smoothing that over.

- **Fixed (parser + printer): `blockaddress` with a numeric label.**
  llvmkit stringified the slot id and looked for a block literally *named*
  `"5"` — which no unnamed block is — so `blockaddress(@f, %5)` could never
  resolve. Upstream keeps the two spellings apart as `ValID::t_LocalID` /
  `t_LocalName`, and llvmkit now does too. A `blockaddress` naming the
  function it appears in resolves through that function's own state
  (upstream's `BlockAddressPFS`), so a label below the reference works; one
  naming a function whose body has closed reports
  `cannot take address of numeric label after the function is defined`,
  because the numbering is gone. Deferred `blockaddress` resolution moved
  from end-of-module to the target function's close, where its labels are
  still numbered. The printer half was broken too: it printed the target
  block without the target *function's* slot numbering, so an unnamed block
  came out as `%<unnumbered>` and the module could not round-trip.

- **Fixed (parser): a comdat may be used before it is defined**, and
  redefining one is now an error. Every site ran `get_or_insert_comdat`, so a
  *use* silently created the comdat and a second `$c = comdat ...` was
  silently accepted. Ports `LLParser::getComdat` and the
  `!ForwardRefComdats.erase(Name)` guard in `parseComdat`, adding
  `use of undefined comdat '$x'` and `redefinition of comdat '$x'`.

- **Fixed (`llvmkit-ir`): a function's address carries the function's own
  address space.** `FunctionValue::as_global_constant_ptr` hard-coded 0,
  where `GlobalValue::getType` builds the pointer from the value's own
  address space.

- **Fixed (`llvmkit-ir`): a `common` global may be initialized with a zero
  aggregate.** The verifier asked whether the initializer was a zero *scalar*
  — `Int(0)`, `Float(0)`, `null` — where upstream's
  `Verifier::visitGlobalVariable` asks `Constant::isNullValue`, which is true
  of `zeroinitializer` at any type. `common global [10 x %struct] zeroinitializer`
  is what clang emits, and llvmkit rejected it. The predicate is now one port
  of `isNullValue`, shared with the shufflevector folder that already had it.
  Reachable only once forward global references landed: the fixture that
  exposes it, `test/Assembler/2010-02-05-FunctionLocalMetadataBecomesNull.ll`,
  had never got past parsing.

- **Fixed (parser): phi incoming values take the general value path.**
  `parsePHI` reads each incoming with `parseValue`, so `[ @g, %bb ]`,
  `[ 1.5, %bb ]` and any constant expression are legal there. llvmkit had a
  hand-rolled token match accepting only locals, integer literals,
  `zeroinitializer`, `null`, `undef` and `poison`, and it silently rewrote a
  forward-referenced `undef` incoming to zero at end of function.
  `phi i32 [ 0, %a ], !dbg !1` also parses now, matching the trailing-metadata
  rule the index lists already follow.

- **Fixed (parser): three `PerFunctionState` diagnostics that were llvmkit's
  own wording.** `'%x' defined with type 'T' but expected 'U'` and
  `'%x' is not a basic block` (`checkValidVariableType`),
  `instruction forward referenced with type '<T>'` (`setInstName`), and
  `instructions returning void cannot have a name`. An undefined *label* is
  now reported as `use of undefined value '%missing'`: upstream keeps blocks
  in the same `ForwardRefVals` map as values, and `finishFunction` has only
  the one message. Which name a leftover diagnostic names is upstream's too —
  the lexicographically smallest, then the smallest slot number.

### Parser accepts IR that `llvm-as` accepts (clause order, pointer compares)

Second wave of the `LLParser` parity program. These are inputs LLVM produces
and llvmkit rejected.

- **Breaking (parser + printer): `dso_local` now precedes visibility and DLL
  storage class.** `LLParser::parseOptionalLinkage` reads linkage, then
  dso-locality, then visibility, then DLL storage; `AsmWriter::printFunction`
  writes the same order. llvmkit's `declare` / `define` paths read visibility
  and DLL storage *first*, and its own writer emitted that same wrong order —
  so llvmkit round-tripped itself while failing to parse `define dso_local
  hidden void @f()`, the spelling LLVM itself emits. The global path was
  already correct; all three now share one helper mirroring
  `parseOptionalLinkage`, which also brings its cross-clause rejection,
  `dso_location and DLL-StorageClass mismatch`.

- **Fixed (parser): `icmp` accepts pointer operands.** `parseCompare`'s guard
  is `!isIntOrIntVectorTy() && !isPtrOrPtrVectorTy()`, so `icmp eq ptr %a, %b`
  is ordinary IR. llvmkit narrowed both operands to `IntValue<IntDyn>` on the
  scalar path, which no pointer satisfies, and rejected every pointer
  comparison. Pointers now take the erased path, as vectors already did —
  `IrBuilder::int_cmp_erased` accepted them all along.

- **Fixed (parser): the two compare-operand diagnostics.** `icmp requires
  integer operands` and `fcmp requires floating point operands`, reported
  against the operand type as upstream reports them, replacing builder-error
  passthrough.

- **Fixed (parser + IR model): fast-math flags on `select`, `phi`, `fptrunc`
  and `fpext`.** `LLParser::parseInstruction` eats flags for these four
  keywords before dispatching, then applies them to the result. llvmkit could
  not parse `select fast`, `fptrunc contract` or `fpext reassoc` at all, and
  **silently discarded** the flags it did parse on `phi`, so `phi nsz double`
  round-tripped as a plain `phi`. `SelectInstData`, `PhiData` and `CastOpData`
  gain an `fmf` slot, the AsmWriter prints it, and new
  `IrBuilder::select_erased_with_fmf` / `fp_trunc_dyn_with_fmf` /
  `fp_ext_dyn_with_fmf` plus `set_fast_math_flags` on the phi handles carry it
  (the plain forms delegate with empty flags, so no existing call site
  changes). Upstream's two rejections come with them: `fast-math-flags
  specified for select without floating-point scalar or vector return type`
  and its `phi` twin.

- **Fixed (parser): the `call` fast-math rejection uses upstream's wording.**
  Its comment already quoted `fast-math-flags specified for call without
  floating-point scalar or vector return type` while the code emitted a
  reworded `expected …` message.

- **New: `Type::is_float_or_float_vector`** — mirrors `isFPOrFPVectorTy`, the
  predicate that decides whether an instruction is an `FPMathOperator` and may
  therefore carry fast-math flags.

- **Breaking (parser): numbered slots may skip ahead.** `LLParser::checkValueID`
  rejects a slot only when it goes *backwards* (`ID < NextID`); a gap is legal
  and the writer renumbers it away. llvmkit required each numbered instruction,
  argument and block label to equal the frontier exactly, so it rejected every
  spelling in `test/Assembler/skip-value-numbers.ll` — `%10 = add i32 1, 2` as
  a function's first instruction, `define i32 @args(i32 %0, i32 %10, i32 %20)`,
  and skipped-ahead block labels — all of which `llvm-as` accepts. The five
  `(kind, prefix)` message forms are upstream's:
  `instruction expected to be numbered '%11' or greater`, and the `argument`,
  `label` (deliberately sigil-less), `global` and `function` variants.

  Two llvmkit tests asserted the *opposite* of the fixture they cited —
  `skip-value-numbers-invalid.ll` rejects going backwards, not skipping ahead.
  They are replaced by ports of that fixture and of its positive sibling.

- **Fixed (parser): bare `comdat`.** `LLParser::parseOptionalComdat` lets
  `comdat` with no parenthesised name borrow the symbol's own name. llvmkit
  rejected the bare form on globals (`expected explicit comdat($name)`) and,
  on functions, silently built a comdat named `""`. The one case upstream
  *does* reject — an unnamed symbol, which has no name to borrow — now reports
  `comdat cannot be unnamed`, ported from
  `test/Assembler/unnamed-comdat.ll`.

- **Fixed (parser + printer): `declare !dbg !0 void @f()`.**
  `LLParser::parseDeclare` reads metadata attachments written *before* the
  header and applies them once the function exists; llvmkit went straight to
  linkage, so the form did not parse. The printer half was wrong in the same
  place: `AssemblyWriter::printFunction` emits a declaration's attachments
  directly after the `declare` keyword and a definition's after the header,
  both **space**-separated, where llvmkit emitted a comma-separated suffix for
  both. `fmt_metadata_attachments` now takes the separator, as upstream's
  `printMetadataAttachments` does — globals and instructions keep `", "`.

- **Breaking (parser): an all-constant `select` stays an instruction.**
  `LLParser::parseSelect` ends in an unconditional `SelectInst::Create`, and
  the parser's own builder already uses `NoFolder` — but llvmkit folded a
  constant `select` away in the parse arm itself, bypassing both. So
  `%r = select i1 true, i32 5, i32 5` vanished from the printed module, and a
  trailing `!dbg` on it landed on whatever instruction came before, since the
  attachment path takes the block's last instruction and a folded select adds
  none. LLVM 22 removed `select` constexprs outright (llvmkit itself reports
  `select constexprs are no longer supported`), so there is no constant form
  for a parser-side fold to produce.

  The test that covered this cited `test/Assembler/ConstantExprFoldSelect.ll`
  while asserting its `CHECK` line from a bare parse — but that fixture's
  `RUN` line is `opt -S -passes=instsimplify`, so the folding it checks is a
  *pass* result. It now asserts what the parser does; the folding half was
  already covered directly through the API in
  `llvmkit-ir/tests/constant_fold.rs`.

- **Fixed (parser): an index list stops at a trailing metadata attachment.**
  `getelementptr i32, ptr %p, i64 1, !dbg !0` did not parse — the index loop
  ate the comma and tried to read `!dbg` as a type. `extractvalue` and
  `insertvalue` had the same hole. Upstream breaks out of the loop on a
  `MetadataVar` (`LLParser::parseIndexList`, and inline in
  `parseGetElementPtr`); llvmkit now restores the comma for the trailing-
  metadata handler, the backtrack `parseAlloc`'s equivalent already used.
  This shape appears in essentially every `clang -g` module.

- **Fixed (parser): `memory(argmem : read)` parses, and its six diagnostics
  are upstream's.** `LLParser::parseMemoryAttr` puts the lexer in
  `setIgnoreColonInIdentifiers` mode so the colon is a separator, not a label
  terminator — whitespace around it is insignificant. llvmkit matched
  locations by looking for a *label* token instead, which requires the colon
  glued to the word, so the spaced spelling failed and `expected ':' after
  location` was unreachable. The routine now mirrors upstream's loop and
  emits `expected '('`, `expected ':' after location`, `expected memory
  location (argmem, inaccessiblemem, errnomem) or access kind (none, read,
  write, readwrite)`, `expected access kind (none, read, write, readwrite)`,
  `default access kind must be specified first` and `unterminated memory
  attribute`. Five of the eight splits of
  `test/Assembler/memory-attribute-errors.ll` are ported; the three that hinge
  on a misspelled keyword are blocked on llvmkit's lexer reporting
  `unknown keyword '...'` where upstream defers to the parser, and are
  recorded against the lexer-parity work.

  That printer fix reaches further than the `declare` form it was written for:
  llvmkit also emitted `define void @f(double %x), !dbg !0 {` for
  *definitions*, and the comma spelling appears nowhere in upstream's test
  suite — `printFunction` writes `define … ) !dbg !0 {`. A checked-in corpus
  expectation had encoded the wrong form, and is corrected here.

### Parser diagnostics are rendered with upstream's exact wording

First wave of the `LLParser` 1:1 parity program. This one is about what a
rejection *says*, not which inputs are rejected — with one exception, noted
last, that a message-parity test uncovered.

- **Breaking (parser): `ParseError::Message` joins `ParseError::Expected`.**
  Every prose diagnostic used to route through `Expected`, whose `Display` is
  `expected {expected}`, so llvmkit printed `expected udiv constexprs are no
  longer supported` where `llvm-as` prints the sentence bare. `Message`
  renders its text verbatim and is now used by the ~100 sites whose upstream
  wording does not begin with `expected ` — the removed-constexpr family, the
  `ptrauth` operand checks, the alias linkage and visibility rules, the
  `getelementptr` constant-expression checks, and `intrinsic can only be used
  as callee` among them. Matching on `ParseError::Expected` for one of those
  now matches `ParseError::Message` instead.

- **Fixed (parser): 21 messages said `expected` twice.** Payloads that already
  began with `expected ` were rendered by a `Display` that prepends the word
  again, so `expected '(' in constant ptrauth expression` reached users as
  `expected expected '(' in constant ptrauth expression`. The redundant prefix
  is gone from the stored payload; the rendering is unchanged and now correct.

- **Fixed (lexer): seven messages reworded to upstream's text.**
  `unterminated /* */ block comment` → `unterminated comment`;
  `invalid value number (does not fit in u32)` → `invalid value number (too
  large)`; the integer-width message drops the interpolated bounds to match
  `bitwidth for integer type out of range`; the hex-float overflow messages
  spell the type lowercase (`half`, `bfloat`) instead of `Debug`-formatting it;
  and `UnterminatedQuotedName` gains a `Display` covering upstream's three
  spellings across the five kinds. That last one reproduces an upstream quirk
  deliberately: `LLLexer::LexVar` serves both `@"…"` and `%"…"`, so an
  unterminated *local* name really does report `end of file in global variable
  name`.

  Structured fields are untouched — the width and limit are still on the
  error for callers that want them, they simply no longer appear in prose that
  upstream writes without them.

- **Breaking (lexer): `i8388609` and wider are rejected, as upstream rejects
  them.** `INT_TY_MAX_BITS` read `(1 << 24) - 1` while its doc comment claimed
  to mirror `IntegerType::MAX_INT_BITS`, which is `1 << 23`. llvmkit therefore
  accepted integer types up to `i16777215` — `llvm-as` refuses everything above
  `i8388608`, and `test/Assembler/invalid-inttype.ll` exists to pin exactly
  that boundary. The parser separately reported the wrong limit in its own
  message (`(1..=16777215)`) while applying the correct check, so the
  diagnostic contradicted the rejection that produced it. Both now read the
  one constant.

- **Tests assert rendered text, not payload fields.** `parse_error.rs`
  previously documented the opposite policy ("structural identity, not string
  identity, to keep wording flexibility"), which is what let all of the above
  hide: every fixture matched a variant's field, so no test could observe the
  `expected ` prefix that `Display` added afterwards. New
  `crates/llvmkit-asmparser/tests/parser_diagnostics.rs` ports four upstream
  negatives whose `FileCheck` lines pin the message text
  (`invalid-c-style-comment0.ll`, `invalid-inttype.ll`, `hex-float-overflow.ll`,
  `internal-hidden-alias.ll`), and the existing rejection helpers now compare
  `err.to_string()`.

### Specialized `DI*` field *values* are validated against upstream's tables

- **Breaking (parser): a `DW_*` / `DIFlag*` / kind spelling upstream does not
  know is now rejected.** Every enum-ish field was an unchecked string, so
  llvmkit parsed `tag: DW_TAG_bogus`, `flags: DIFlagBogus`,
  `checksumkind: CSK_BOGUS` and eleven other families that `llvm-as` refuses.
  Each now reports upstream's own wording from the matching
  `LLParser::parseMDField` overload — `invalid DWARF tag '...'`,
  `invalid debug info flag '...'`, `invalid checksum kind '...'`, and so on.

- **Breaking (parser): range, null, empty and boolean checks.**
  `value for '<field>' too large, limit is <max>` (the limit is the one the
  field's declared type carries — `LineField` is `UINT32_MAX`, `ColumnField`
  `UINT16_MAX`), the signed `too small` twin, `'<field>' cannot be null` for an
  `MDField(AllowNull=false)`, `'<field>' cannot be empty` for an
  `MDStringField(EmptyIs::Error)`, and `expected 'true' or 'false'`.

- **`SpecializedMetadataKind::declared_fields` replaces `fields` /
  `required_fields`.** One table now carries the accepted spelling, the value
  grammar, and the required flag for all 239 fields across 30 classes, because
  all three come from the same upstream `VISIT_MD_FIELDS` line and separate
  tables could drift. New `MetadataFieldKind` (one variant per `parseMDField`
  overload) and `SpecializedMetadataField`. `required_fields` survives as an
  iterator over the same table.

  `MetadataFieldKind` is deliberately not `#[non_exhaustive]`: the parser
  matches on it to pick a validation, and a catch-all arm would let a field
  kind added by a future LLVM bump parse *unchecked* — reintroducing the exact
  divergence this closes. Exhaustiveness makes that a compile error instead.

- Fixed before shipping: the `DIFixedPointType` `kind` table was written
  `Unsigned`/`Signed`/`Rational` and would have **rejected valid IR** —
  upstream's spellings are `Binary`/`Decimal`/`Rational`. These three families
  come from C++ enums rather than a `.def`, so `dwarf_def_drift.rs` cannot
  cover them; a test pins them against the lexer's word lists instead.

- Known residual, recorded rather than papered over: for the three families
  `LLLexer` matches as exact words (`emissionKind`, `nameTableKind`, fixed-point
  `kind`), an unknown spelling is rejected by llvmkit's *lexer*, where upstream
  reaches `expected <kind>` in the parser. Same verdict, different layer.

### The specialized metadata set is complete: all 32 `Metadata.def` leaves

- **Fixed: `!DILexicalBlock(...)` and thirteen sibling classes did not parse.**
  llvmkit modelled 18 of upstream's 32 `HANDLE_SPECIALIZED_MDNODE_LEAF` entries
  (`llvm/IR/Metadata.def`); the rest failed with "expected specialized metadata
  kind". `DILexicalBlock` and `DILexicalBlockFile` appear in essentially every
  `-g` build, so this was not an exotic gap. Added: `GenericDINode`,
  `DISubrangeType`, `DIGenericSubrange`, `DIFixedPointType`, `DIStringType`,
  `DILexicalBlock`, `DILexicalBlockFile`, `DICommonBlock`, `DIMacro`,
  `DIMacroFile`, `DILabel`, `DIObjCProperty`, `DIImportedEntity`, `DIAssignID`.

- Each carries the accepted- and required-field tables from its
  `LLParser::parse*` `VISIT_MD_FIELDS` block, so all three field rejections
  apply to them immediately. The tables were generated from the vendored
  `LLParser.cpp` rather than hand-typed, then diffed back against it.

- `DIAssignID` gets upstream's one structural rule: `LLParser::parseDIAssignID`
  rejects a uniqued node before reading the parens, so only
  `distinct !DIAssignID()` is accepted.

- `SpecializedMetadataKind::ALL` is now 32 entries, pinned against
  `Metadata.def`'s leaf list by a completeness test.

### `!DIExpression(...)` with a body now parses

- **Fixed: a non-empty `DIExpression` was unparseable.** Only `!DIExpression()`
  worked; every form clang actually emits — `!DIExpression(DW_OP_deref)`,
  `!DIExpression(DW_OP_LLVM_fragment, 0, 32)`, `!DIExpression(DW_OP_plus_uconst,
  8)` — was rejected, because the specialized-node loop demanded `name: value`
  pairs. `DIExpression` is the one node upstream routes away from
  `PARSE_MD_FIELDS`, to `parseDIExpressionBody` (`LLParser.cpp`): its body is a
  positional list. Ported from `test/Assembler/diexpression.ll`.

- **Breaking (`llvmkit-ir`): `SpecializedMetadataNode` gains a body ADT.** New
  `SpecializedMetadataBody` (`Fields` | `Expression`) and
  `DwarfExpressionOperand` (`Operation` | `Literal`), with
  `SpecializedMetadataNode::body` / `expression_operands` /
  `with_expression_operands`. Two shapes because upstream has two; one enum
  rather than two vectors keeps "a `DIExpression` carrying named fields"
  unrepresentable (D1). `fields()` is unchanged for every other kind and now
  returns an empty slice for a `DIExpression`.

- Operands keep their **source spelling** rather than the `uint64_t` encodings
  upstream stores via `dwarf::getOperationEncoding`: the `Dwarf.def` tables are
  still unmodelled, and `AsmWriter.cpp`'s `writeDIExpression` prints a known
  operation back by name anyway, so the written form is what round-trips. One
  deliberate divergence follows — an unrecognised `DW_OP_*` round-trips here
  where upstream rejects it. Recorded in `docs/future-work.md`.

### Specialized `DI*` metadata: fields are validated against their node class

- **Breaking (parser): `!DILocation(lien: 3)` no longer parses.** The
  specialized-node loop accepted *any* `LabelStr` as a field key, so llvmkit
  parsed debug metadata that `llvm-as` rejects. It now makes all three
  rejections `LLParser`'s `PARSE_MD_FIELDS` macro makes (`LLParser.cpp`):
  `invalid field '<name>'` for a key the class does not declare, `field
  '<name>' cannot be specified more than once` (from `LLParser::parseMDField`'s
  `Result.Seen` guard), and `missing required field '<name>'` (`REQUIRE_FIELD`,
  reported against the closing `)` exactly as upstream does). Three new
  `ParseError` variants carry the node kind, the field, and the location;
  `ParseError` is `#[non_exhaustive]`, so the additions are not themselves a
  break.

- **Added: `SpecializedMetadataKind::fields` / `required_fields` /
  `accepts_field` / `ALL`.** Each row ports the matching
  `LLParser::parseDI*`'s `VISIT_MD_FIELDS` block. `DiExpression`'s table is
  deliberately empty — `LLParser::parseDIExpression` uses no `VISIT_MD_FIELDS`
  at all, because a `DIExpression` body is a positional `DW_OP_*` list rather
  than `name: value` pairs.

- **Fixed: `flags:` / `spFlags:` accept a `|`-joined disjunction.**
  `flags: DIFlagPublic | DIFlagStaticMember` previously failed to parse
  outright. It is kept as the joined source text, which is byte-for-byte what
  `AsmWriter.cpp`'s `printDIFlags` emits (`ListSeparator(" | ")`); modelling
  `DINode::DIFlags` as an actual bitmask stays deferred.

- Four llvmkit-authored test fixtures were invalid IR that only the lax parser
  had accepted — a `DICompileUnit` without `file:` (three of them, one being a
  deviation from the upstream fixture it claims to derive from) and two
  `DIDerivedType` / `DICompositeType` nodes without `tag:`. All corrected.

### API idiomatics program (Rust API Guidelines sweep)

A coordinated set of breaking renames and reshapes bringing the whole public
surface in line with the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
(C-CASE, C-GETTER, C-CONV, C-CUSTOM-TYPE, C-ITER, C-BUILDER, ...). Pre-1.0,
no users: one coherent break instead of a drip.

Executed wave-by-wave, and the bullets below run **in wave order** — the
program's own narrative, from renaming what things are called (W1–W3) through
what their signatures promise (W4–W5), what types the API models instead of
raw scalars (W6–W8), what its reads and constructions hand back (W9–W10), and
finally the trait floor and the crate surfaces (W11a–W12). Landing order on the
branch differed, because several waves ran in parallel; each bullet names its
wave so a commit can be traced back to it.

#### Changed

- **Breaking (W1): strict RFC-430 acronym casing on type names.**
  `IRBuilder` → `IrBuilder`, `IRBuilderFolder` → `IrBuilderFolder`,
  `CFGAnalyses` → `CfgAnalyses`, `ICmpInst` → `IcmpInst`, `FCmpInst` →
  `FcmpInst`, `FNegInst` → `FnegInst`, `VAArgInst` → `VaArgInst`, `ICmpFlags`
  → `IcmpFlags`, `ZExtFlags` → `ZextFlags`, `UIToFpFlags` → `UiToFpFlags`,
  and the crate-internal payload/id companions (`FcmpInstData`, `FnegInstData`,
  `VaArgInstData`, `VaArgInstId`); asmparser's `GVarFlags` becomes
  `GlobalVariableFlags` (casing + the full-words law). Aligns with the
  existing `IrError` / `IrResult` / `CfgUpdate` spellings.

- **Breaking (W1b): the same RFC-430 casing, now on enum variants and
  macro-declared types.** W1 found its targets by grepping declaration
  keywords, which sees neither `enum` bodies nor types minted inside
  `decl_binop_handle!` / `decl_cast_handle!` / `decl_value_id!` /
  `decl_struct_kind!` / `decl_exact_flags!` — so two whole categories survived
  it. Both are converted now: **258 symbols, 2459 edits.** The opcode and
  instruction-kind variants (`Opcode::ICmp` → `Icmp`, `CastOpcode::ZExt` →
  `Zext`, `InstructionKind::AShr` → `Ashr`, `BinaryOpcode::UDiv` → `Udiv`,
  `FAdd` → `Fadd`, `VAArg` → `VaArg`, `AtomicRMW` → `AtomicRmw`, asmparser's
  `FPToSI` / `UIToFP` / `FPExt` → `FpToSi` / `UiToFp` / `FpExt`); the intrinsic
  and `atomicrmw` mnemonics (`SMax` → `Smax`, `USubSat` → `UsubSat`, `BSwap` →
  `Bswap`, `VScale` → `Vscale`, `FMaximum` → `Fmaximum`, `UIncWrap` →
  `UincWrap`); attribute, linkage and layout variants (`StrictFP` → `StrictFp`,
  `UWTable` → `UwTable`, `VScaleRange` → `VscaleRange`, `WeakODR` → `WeakOdr`,
  `LinkOnceODR` → `LinkOnceOdr`, `XCoff` → `Xcoff`, `ATT` → `Att`);
  `ConstantData::DSOLocalEquivalent` → `DsoLocalEquivalent`; the 18
  `SpecializedMetadataKind::DI*` node kinds → `Di*`; and the macro-declared
  handles, flag structs and markers (`AShrInst` → `AshrInst`, `ZExtInst` →
  `ZextInst`, `FAddInst` → `FaddInst`, `UDivFlags` → `UdivFlags`,
  `AtomicRMWInst` / `Data` / `Id` / `Config` / `Flags` / `BinOp` →
  `AtomicRmw*`, `GlobalIFunc*` → `GlobalIfunc*`, the `BFloat` float-kind marker
  → `Bfloat`), plus the two analysis-side carriers `EquivalentICmp` →
  `EquivalentIcmp` (`constant_range.rs`) and `SelectPatternNaNBehavior` →
  `SelectPatternNanBehavior` (`select_pattern.rs`).
  Two names also shed an abbreviation, exactly as `GVarFlags` did
  in W1: `NamedMDNode` → `NamedMetadataNode` (its three sibling types already
  spell `NamedMetadata` in full) and `UseListOrderBBRecord` →
  `UseListOrderBbRecord` (matching asmparser's existing `Keyword::UselistorderBb`).

  **Printed IR is byte-identical.** Every `.ll` spelling lives in a string
  literal, not in a variant name — `SpecializedMetadataKind::DiLocation` still
  prints `DILocation`, `AtomicRmwBinOp::UincWrap` still prints `uinc_wrap` — so
  the round-trip corpus and the byte-lock examples are untouched. Deliberately
  left alone: upstream C++ citations in comments (`AtomicRMWInst`,
  `Instruction::FPToSI`, `NamedMDNode *`), `.ll` keyword strings,
  `PunctKind::LBrace`-style Left/Right punctuation names, and
  `PhiViolation::NotAPredecessor` — in those the second capital opens an
  ordinary word, not an acronym.

- **Breaking (W2): the `build_` prefix is gone from every `IrBuilder` method.**
  All 265 emitters lose the C++ `CreateAdd`-heritage prefix: `build_int_add` →
  `int_add`, `build_ret` → `ret`, `build_br` → `br`, `build_call` → `call`,
  `build_gep` → `gep`, and so on — call sites now read
  `b.int_add(lhs, rhs, "sum")?`, and the `IrBuilder` and `SsaBuilder` layers
  spell their instruction vocabulary identically
  (`ssa.ins()?.int_mul(a, b, "x")`). Folded-in normalizations: the two
  `float_neg` stragglers join the `fp_` family as `fp_neg` / `fp_neg_fmf`
  (the FMF variant also swaps its misleading `_with_flags` suffix for the
  `_fmf` the rest of the fast-math family uses); the abbreviated `vec_*` /
  `arr_*` typed vector/array emitters become `vector_*` / `array_*` (full-words
  law); and the confusing near-duplicate splat pair is resolved as
  `vector_splat` (typed, was `build_vec_splat`) vs `vector_splat_dyn` (erased,
  was `build_vector_splat`). Chainable builders keep their `.build()`
  terminals (C-BUILDER); test function names keep their historical spellings.

- **Breaking (W3): lookups are bare nouns; `get_` is reserved for
  get-or-insert.** `Module::get_global` → `global`, `get_alias` → `alias`,
  `get_ifunc` → `ifunc`, `get_comdat` → `comdat`, `function_by_name::<R>` →
  `function::<R>`, `function_by_name_dyn` → `function_dyn` (C-GETTER). The
  named-struct pair untangles: the get-or-create former `named_struct`
  becomes `get_or_insert_named_struct`, freeing `named_struct(name)` for the
  pure lookup; the bespoke `get_or_set_named_struct_body::<S>` becomes
  `get_or_insert_struct_of::<S>`. Also de-prefixed: `Type::get_undef`/
  `get_poison` → `undef`/`poison`; the free functions `splat_value`,
  `select_pattern`, `underlying_object{,_aggressive,s,s_for_code_gen}`,
  `constant_data_array_info`, `constant_string_info`, `string_length`;
  `DemandedBits::{demanded_bits, operand_demanded_bits}`; the analysis
  managers' `result::<A>` / `cached_result::<A>`; asmparser
  `NumberedValues::next_unused_id` (was `get_next`);
  `AttributeStorage::or_default_mut` (was `get_mut_or_default`).
  The `get_or_insert_*` family keeps its std-consistent names.

- **Breaking (W3 follow-up): `MemoryEffects::get_mod_ref` → `mod_ref`.**
  One public accessor was missed by the sweep above and surfaced in the
  documentation audit that followed. It sat beside its own un-prefixed
  sibling `with_mod_ref`, so the pair read `get_mod_ref` / `with_mod_ref`.
  Mirrors `MemoryEffectsBase::getModRef` (`ModRef.h`); upstream's camelCase
  getter becomes a bare-noun Rust accessor, exactly as the other `get_*`
  methods in this wave did (C-GETTER).

- **Breaking (W3): conversion-name honesty (C-CONV).**
  `Value::as_const_int` → `to_const_int` (it allocates an `ApInt`);
  `AtomicOrdering::to_ir_string` and `IntPredicate`/`FloatPredicate::name`
  unify on `as_str`; the `IsValue` widening accessor and every inherent
  twin rename `into_erased` → `as_erased`, joining the `as_dyn`/`as_view`
  family (the operand-lift trait method `into_erased_value` is unchanged);
  `BlockCursor::next` → `step` (it is deliberately not an `Iterator`; the
  old name invited `for`-loop attempts that failed confusingly), and
  `WorklistScope::next` → `step` follows it for the same reason (that one
  rode along with W5).
  `set_data_layout` is now the single, infallible setter taking a parsed
  `DataLayout`; the string-parsing overload is gone — parse explicitly with
  `DataLayout::parse(...)` (fallibility lives where the failure is).

- **Breaking (W4): signature generics tell the truth about ownership
  (C-GENERIC).** Names that are stored take `Into<String>` — the
  `add_global` family (was `AsRef<str>` + a hidden copy), the chainable
  call-builders' `.name(..)`, `PassPipelineTextName::try_new`; conditional
  get-or-insert keys stay borrow-generic (`get_or_insert_comdat` is now
  `AsRef<str>`). `get_or_insert_intrinsic_declaration_by_id` threads
  `Into<Box<[Type]>>` through (no forced `.to_vec()`);
  `append_block_with_named_params` takes any
  `IntoIterator<Item = (Type, impl Into<String>)>` instead of a
  `&[(Type, &str)]` borrow sandwich; `StructLayoutInfo::new` and
  `PassPipelineElement::new` accept `Into<Vec<u64>>` / `IntoIterator`.
  `Module::global_builder` accepts `impl Into<Type>` like its siblings.
  asmparser: `parse_type`, `parse_type_at_beginning`,
  `parse_constant_value`, `parse_summary_index_assembly` now take
  `impl AsRef<[u8]>` like the rest of the entry points; the
  `slots: Option<&SlotMapping>` mode parameters split into explicit
  `*_with_slots` twins; the redundant `parse_assembly_string` is gone
  (`parse_assembly` already accepts `&str`).

- **Breaking (W5): booleans become methods, flag structs, and `Signedness`
  (C-CUSTOM-TYPE).** Type constructors split instead of taking mode bools:
  `vector_type(elem, n)` / `scalable_vector_type(elem, n)` (was
  `vector_type(elem, n, scalable)`), `struct_type(elements)` /
  `packed_struct_type(elements)`, and — fixing the `fn_` abbreviation too —
  `function_type(ret, params)` / `variadic_function_type(ret, params)`
  (+ `_no_parameters` twins; were `fn_type(.., is_var_arg)` /
  `fn_type_no_params`). Builder toggles are zero-arg: `GlobalBuilder::
  constant()` / `externally_initialized()` (plus a `set_/clear_` pair on the
  `GlobalVariable` view), `InlineAsmOptions::side_effects()` /
  `align_stack()` / `unwind()` (the odd `with_can_unwind(bool)` is gone),
  `SpecializedMetadataNode::distinct()`, and `SpeculationOptions::
  without_variable_info()` / `ignoring_ub_implying_attrs()` (upstream's
  defaults are the defaults; the pair is the clearing side).
  `is_safe_to_speculatively_execute_with_variable_replaced` takes
  `SpeculationOptions` instead of a bare bool. `ApIntSignedness` is renamed
  `Signedness` and replaces every `is_signed` / `sign_extend` /
  `for_signed` bool (`const_int_raw`, `constant_fold_integer_cast`,
  `ConstantRange::from_known_bits`, `compute_constant_range{,_including_
  known_bits}`). `KnownBits` transfer functions take the per-opcode flag
  structs the crate already ships — `add_with_flags(.., AddFlags)`,
  `sub_with_flags(.., SubFlags)`, `shl_with_flags(.., ShlFlags,
  ShiftAmountKnowledge)`, `lshr`/`ashr` with `LShrFlags`/`AShrFlags`,
  `udiv`/`sdiv_with_exact(.., UDivFlags/SDivFlags)`, and
  `compute_for_add_sub(AddSubOperation, OverflowFlags, ..)` — which also
  kills a live footgun: `shl_with_flags` used to take `(nuw, nsw)` in the
  opposite order from `add_with_flags`'s `(nsw, nuw)`. New public enums:
  `AddSubOperation`, `ShiftAmountKnowledge` (an analysis-side fact no `.ll`
  keyword spells, deliberately not part of the IR flag structs).

- **Breaking (W6): `NamedMetadataId<B>` + `NamedMetadataName` replace the raw
  `usize`/`String` named-metadata surface (D7).**
  `Module::get_or_insert_named_metadata` mints a module-tagged, branded
  `NamedMetadataId<B>` instead of a bare list index, and
  `named_metadata_add_operand` takes that id — a foreign id is
  `Err(IrError::ForeignNamedMetadataId)` (new variant), replacing the old
  index's out-of-range `UnknownMetadataSlot` arm (an in-range foreign index
  used to be undetectable in principle; the tag makes it impossible). New
  bare-noun lookup `Module::named_metadata(&NamedMetadataName) ->
  Option<NamedMetadataId<B>>` and clone-out reader
  `Module::named_metadata_get(id) -> Option<NamedMetadataNode<B>>` (`None`
  only for a foreign id; the node type is spelled with W1b's full-word name
  here and below). Node names are the new `#[non_exhaustive]`
  `NamedMetadataName` enum spelling the well-known upstream set
  (`llvm.module.flags`, `llvm.dbg.cu`, `llvm.ident`, `llvm.linker.options`,
  ...) as variants with `Custom(String)` for the open rest;
  `From<&str>`/`From<String>` keep call sites one-liners.
  `NamedMetadataNode::new` takes `impl Into<NamedMetadataName>`, `name()` returns
  `&NamedMetadataName`, and the printed spelling moves to `name_str()`.
  Printed `.ll` output is unchanged.

- **Breaking (W7): typed module flags — `ModuleFlagBehavior` /
  `ModuleFlagKey` / `ModuleFlagEntry`, module accessors, and the verifier
  port (C-CUSTOM-TYPE).** New `module_flags` module mirroring
  `Module::ModFlagBehavior` (`Module.h`, discriminants 1–8 with
  `from_raw`/`raw` spelling `isValidModFlagBehavior`'s range check) and a
  `#[non_exhaustive]` `ModuleFlagKey` naming the 27 well-known keys with
  their exact `lib/IR/Module.cpp` spellings (`"Dwarf Version"`,
  `"PIC Level"`, `"wchar_size"`, `"CG Profile"`, ...), `Custom(String)` for
  the open rest, and `default_behavior()` giving the `Module.cpp` setter
  pairing (`setPICLevel`→`Min`, `setUwtable`→`Max`, `setCodeModel`→`Error`,
  `setSDKVersion`→`Warning`, ...; `None` where `Module.cpp` has no setter).
  `Module` gains `add_module_flag` / `set_module_flag` (append vs
  replace-in-place, mirroring `addModuleFlag`/`setModuleFlag`),
  `module_flag(&ModuleFlagKey)`, and `module_flags()` (which shipped here
  returning `Vec<ModuleFlagEntry>` and became an iterator under W9, below,
  before any of this was released) — all backed by the `llvm.module.flags` named node
  as ordinary `!{i32 behavior, !"key", value}` tuples, so printed IR and
  the round-trip contract are untouched. The breaking half is the verifier:
  `Verifier::visitModuleFlags` / `visitModuleFlag` /
  `visitModuleFlagCGProfileEntry` are ported (tuple shape, behavior
  validity, `MDString` ID, `min`/`max`/`require`/`append` value
  constraints, ID uniqueness, requirement resolution, the
  `aarch64-elf-pauthabi-*` pairing, and the `wchar_size` /
  `Linker Options` / `SemanticInterposition` / `CG Profile` per-key
  checks), so `verify` now rejects malformed `!llvm.module.flags` it
  previously accepted — nine new `VerifierRule::ModuleFlag*` variants. One
  deliberate deviation, documented in the verifier: upstream's `require`
  comparison is uniqued-pointer identity; llvmkit compares structurally
  because tuples and integer constants are not yet uniqued.

- **Breaking (W8): ADT leftovers — the fixed metadata kinds close their drift,
  the `deactivation-symbol` bundle tag is spelled right, and string-attribute
  reading gets typed (C-CUSTOM-TYPE).** Breaking on two counts, both easy to
  miss in an otherwise additive wave: `MetadataAttachmentKind` becomes
  `#[non_exhaustive]`, so an exhaustive downstream `match` on it stops
  compiling, and one operand-bundle tag's printed spelling changes.
  `MetadataAttachmentKind` gains the 17 fixed kinds it was missing
  (`IrrLoop` through `ImplicitRef`, values 24–46 of
  `FixedMetadataKinds.def`), a `fixed_id()` accessor returning the upstream
  kind ID, and `#[non_exhaustive]` (like `AttrKind`) so future upstream kinds
  are additive; a new drift test parses the now-vendored
  `FixedMetadataKinds.def` so the enum can never silently lag again. The
  `OB_deactivation_symbol` operand-bundle tag now parses **and prints** as
  `"deactivation-symbol"` (upstream's `knownBundleName` spelling,
  `lib/IR/LLVMContext.cpp`) — printed IR changes for that tag, which llvmkit
  previously misspelled `"deactivation"`. A source-level `syncscope("system")`
  is preserved as `SyncScope::Named("system")` instead of collapsing to the
  default: upstream seeds only `"singlethread"` and the empty string (the
  canonical `System` name), so `"system"` is an ordinary named scope and now
  round-trips as text. New `gc_strategy` module spells the five built-in GC
  strategy names from `BuiltinGCs.cpp` as `&str` constants (`ERLANG`, `OCAML`,
  `SHADOW_STACK`, `STATEPOINT_EXAMPLE`, `CORECLR`) — constants, not an enum,
  because the upstream registry is designed for out-of-tree collectors;
  `set_gc` still takes any string. New `#[non_exhaustive]` `StrBoolAttrKind`
  enum (the 11 `StrBoolAttr` declarations of `Attributes.td`, with
  `key()`/`from_key()`) and reader
  `FunctionValue::str_bool_attribute(kind) -> Option<bool>` with upstream's
  `getValueAsBool` semantics (`Some(value == "true")`, `None` when absent);
  `Attribute::String { key, value }` construction is unchanged — the
  string-attribute namespace stays open. The attribute drift test now also
  locks the `StrBoolAttr` set against the reader enum and pins the
  `ComplexStrAttr` set to the two `DenormalMode`-typed keys.

- **Breaking (W9): read APIs that allocated a `Vec` return iterators
  (C-ITER).** `FunctionCfg::successors` / `predecessors`,
  `BasicBlock::successors`, `Instruction::debug_records` /
  `InstructionView::debug_records`, `FnReshape::pending_cfg_updates`,
  `AssumptionCache::assumptions`, `DomConditionCache::conditions_for`, and
  `DataLayout::non_standard_address_spaces` / `non_integral_address_spaces`
  no longer build a `Vec` per call. A `for x in …` call site is unaffected; a
  site that asked a `Vec` question (`.is_empty()`, `.len()`, indexing,
  comparing against a `vec![…]`) becomes `.next().is_none()`, `.count()`, or
  `.collect()`. Which of two shapes each one takes follows what the data
  lives behind: the `FunctionCfg` and `DataLayout` methods **borrow**, since
  both types own their tables outright with no interior mutability; the rest
  **snapshot** the `RefCell` they read and hand back an owning iterator,
  which therefore can never be alive across the next edit — with a `use<..>`
  bound keeping `&self` out of the opaque type so the result still chains off
  a borrowed receiver. Three are deliberately **not** `ExactSizeIterator`,
  because they filter and the count is known only after the walk: both
  `DataLayout` address-space methods (which are `+ Clone` instead, being
  borrowing iterators) and `AssumptionCache::assumptions`, which skips slots
  that no longer resolve as instructions — the llvmkit counterpart of
  upstream's dead `WeakVH` entries. Also new: `IntoIterator for
  &AttributeSet` / `&AttributeList` / `&MetadataAttachmentSet`, each yielding
  exactly what that type's `iter()` does, so `for … in &set` works;
  `MetadataAttachmentSet::iter` gains the `ExactSizeIterator +
  DoubleEndedIterator + FusedIterator` bounds its siblings already carried;
  and `Module::attribute_group(id) -> Option<AttributeStorage>` is the point
  lookup most callers of `attribute_groups` actually wanted — the verifier
  and `FunctionValue`'s string-attribute reader use it now instead of
  deep-cloning the whole table once per query. `Module::attribute_groups`
  itself, the whole-table read, became an iterator in the same wave.
  `Module::module_flags` joins them: it shipped one wave earlier than this
  sweep, so the sweep's list never contained it, and it was the last
  `Vec`-returning read API on the public surface. It snapshots, like its
  `RefCell`-backed siblings.

- **Breaking (W10): `LoadBuilder` / `StoreBuilder` / `AllocaBuilder` — one
  spelling per memory op (C-BUILDER).** `load`, `store` and `alloca` each carry
  several *orthogonal* optional knobs, and the flat surface had grown one method
  per combination. Three builders replace those combinations:

  ```rust
  let n = b.load_from(p).volatile().atomic(AtomicOrdering::Acquire)
      .align(align).int::<i32>("n")?;
  b.store_to(v, p).atomic(AtomicOrdering::Release).align(align).build()?;
  let buf = b.alloca_builder(ty).array(n).align(align).name("buf").build()?;
  ```

  A `LoadBuilder` ends in a **typed terminal**, so the result shape is still
  chosen by the caller rather than fixed by the knob spelling (D4 survives the
  move to a builder): `.int::<W>(name)`, `.fp::<K>(name)`, `.pointer(name)`,
  `.typed::<T>(name)` (the `TypedPointerValue` schema route) and
  `.erased(ty, name)`. The marker is each terminal's only generic argument, so
  `.int::<i32>("n")` needs no placeholder turbofish — which is why the terminals
  take `name: &str` rather than the `impl AsRef<str>` the flat forms take
  (explicit generic arguments and `impl Trait` arguments cannot coexist).
  `StoreBuilder` and `AllocaBuilder` have a single `.build()`;
  `AllocaBuilder` takes its name through `.name(..)` and carries the
  `.inalloca()` / `.swifterror()` toggles that were `AllocaFlags`'s job
  (see W12). All three builder types
  are `#[must_use]`, which covers the entry method *and* every setter: a chain
  that forgets its terminal is a warning, not a silent no-op.

  **Retired** — every one of them reachable through a builder:
  `load_volatile`, `load_volatile_with_align`, `store_volatile`,
  `store_volatile_with_align`, `int_load_atomic`, `load_atomic`, `store_atomic`,
  and `alloca_dyn`, whose two `Option` mode-parameters are now `.array(n)` and
  `.addr_space(n)` — present or absent, rather than passed as `None`. The
  `AtomicLoadConfig` / `AtomicStoreConfig` bags the atomic forms took are
  **deleted**: the builder carries that state, and keeping them would leave two
  public spellings for one instruction. `AtomicCmpXchgConfig` /
  `AtomicRmwConfig` are untouched — `cmpxchg` and `atomicrmw` are single
  operations with no orthogonal-knob explosion and keep their config-bag
  spelling.

  **Kept, unchanged:** the plain flats `load`, `load_with_align`, `int_load`,
  `int_load_dyn`, `int_load_with_align`, `fp_load`, `fp_load_dyn`,
  `pointer_load`, `typed_load`, `typed_load_with_align`, `store`,
  `store_with_align`, `typed_store`, `typed_store_with_align`, `alloca`,
  `alloca_with_align`, `array_alloca`, `array_alloca_with_align` and
  `typed_alloca`. The common case does not pay for the general one.

  Two semantics worth stating: `.atomic(ordering)` leaves the sync scope at
  `SyncScope::System` unless `.sync_scope(..)` is also called, in either chain
  order; and an atomic load/store with no explicit `.align(..)` is filled with
  the DataLayout ABI alignment on the way out — never zero, so LangRef's
  non-zero-alignment requirement holds — which is exactly what upstream's own
  builder path produces (`computeLoadStoreDefaultAlign` at construction, with
  `setAtomic` leaving the alignment alone). The retired config bags demanded an
  `Align` in their constructor; nothing else in the tree enforced it, and no
  error variant ever described its absence.

- **Breaking (W11a): the std-trait floor — `Debug` everywhere, `Hash`,
  `FromStr`, `#[must_use]` (C-COMMON-TRAITS).** `Module<B, S>` had no `Debug`
  at all, so `dbg!(&module)` did not compile and no user struct holding a
  module could derive `Debug`. It has one now, hand-written as a **summary** —
  `Module { name, id, functions: N, globals: N, state }` — deliberately *not*
  forwarding to `Display`, which prints the entire `.ll` file. The typestate
  is named (`"Verified"` / `"Unverified"`) through a `TypeId` comparison,
  which is why the impl carries an `S: 'static` bound.

  Seventy more public types across `llvmkit-ir`, `llvmkit-support` and
  `llvmkit-macros` gained `Debug`, taking a scripted sweep of every
  `pub struct` / `pub enum` in those crates from 70 missing to **0 of 387**.
  Derived where it derives cleanly (through `#[derive(Branded)]` wherever a
  brand or marker parameter is involved, so no spurious `B: Debug` bound
  appears); hand-written where a field is a closure, a `dyn` trait object, or
  an associated type with no `Debug` bound — the analysis managers print
  registered/cached counts, `PassInstrumentationCallbacks` prints per-hook
  callback counts, the erased pipelines print member counts, `SsaBuilder`
  prints its function and whether the cursor is positioned,
  `ValueTrackingQuery` prints which facts the query may use. Every impl that
  reaches through a `RefCell` uses `try_borrow`, so printing a structure a
  mutator is holding open degrades to a marker rather than panicking — a
  `Debug` is what a caller reaches for *while* debugging. `ValueCategory`
  gained the full `Debug, Clone, Copy, PartialEq, Eq, Hash` set; it had no
  derives at all.

  `#[must_use]` on the 58 remaining consuming builder methods
  (`FunctionBuilder` 14, `GlobalBuilder` 11, `ValueTrackingQuery` 9,
  `GlobalAliasBuilder` 6, the metadata builders 5, `InlineAsmOptions` 4,
  `GlobalIFuncBuilder` 3, `ConstantExprOptions` 2, plus the pure combinators
  in `fp_class.rs`, `gep_no_wrap_flags.rs` and `instructions.rs`). The
  convention already existed on ~50 siblings and was unevenly applied, so
  `builder.align(a);` used to compile and silently do nothing.

  `IrError` and `DataLayout` gain `Hash` beside the `Eq` they already had:
  errors de-duplicate in a `HashSet`, and a layout is a natural cache key.
  New conversions, with each type's `from_raw` / `as_raw` `const fn` kept as
  the const path: `TryFrom<u8>` for `IntPredicate` / `FloatPredicate`,
  `From<IntPredicate>` and `From<FloatPredicate>` for `u8`, and
  `From<CallingConv>` for `u32`. `TryFrom` reports a new
  `IrError::InvalidDiscriminant { target, value }`.

  **`FromStr` for the keyword types** — thirteen new impls where the crate had
  two, all with `Err = IrError` and a new
  `IrError::InvalidKeyword { target, keyword }` (both variants are additive:
  `IrError` is `#[non_exhaustive]`): `Linkage`, `Visibility`,
  `DllStorageClass`, `DsoLocality`, `ThreadLocalMode`, `UnnamedAddr`,
  `SelectionKind`, `AtomicOrdering`, `CallingConv`, `IntPredicate`,
  `FloatPredicate`, `SyncScope`, and `DataLayout` (which delegates to
  `DataLayout::parse`, so its error stays the specific `InvalidDataLayout`).
  Each `from_str` **inverts the type's own spelling table** — searching a new
  `pub const VARIANTS`, the existing `all()`, or, for `CallingConv`, the whole
  `0..=MAX` id space — rather than carrying a second table that could drift
  from `Display`. For the keyword-optional enums, `""` resolves to the default
  variant, so `parse(display(v)) == v` holds for *every* variant; that
  identity is locked by a per-type in-file drift-lock test (the
  `attribute_td_drift.rs` analogue) whose exhaustive `match` makes a new
  variant a compile error. `SyncScope` is the documented exception: its
  `Display` prints the `syncscope("…")` wrapper, so its `FromStr` takes a bare
  scope *name* and is the inverse of `LLVMContext::getOrInsertSyncScopeID`,
  not of `Display`.

  `llvmkit-support`: `SourceMap` gains `Clone` and a hand-written `Debug` that
  prints byte and line counts rather than the buffer. `Spanned<T>`'s `Ord` /
  `PartialOrd` are now **hand-written span-first**; the derived versions
  compared `value` before `span` (field order), so sorting spanned tokens gave
  token order rather than source order. `Span` gains `contains(u32)` and
  `join(Span)`, both `const fn` — the parsers were hand-rolling both.

  `llvmkit-macros`: `#[derive(Branded)]` accepts opt-in `PartialOrd` and `Ord`
  (field-order lexicographic for structs, declaration-order-then-payload for
  enums, with the variant rank spelled as a `match` rather than an `as` cast,
  and the `Ord: Eq + PartialOrd` supertrait chain checked at the attribute).
  `BlockId` now takes its impls from that derive instead of ~35 lines of
  hand-written ones, and the `decl_value_id!` family — `ValueId` and its typed
  siblings — gains the same lexicographic `(ModuleId, slot)` order, with
  `ModuleId` and `ValueSlot` gaining `PartialOrd`/`Ord` to support it. The
  point is `BTreeMap` / `BTreeSet` keys: a pass that wants deterministic
  output needs a total order that does not vary run to run, which a `HashMap`
  does not give.

  **Also breaking for `.err().expect(…)` callers**: giving the
  terminator-edit handles a `Debug` newly enables `clippy::err_expect` at such
  sites, which are now spelled `.expect_err(…)`.

- **Breaking (W11b): `llvmkit-asmparser` exports its own surface, and its
  errors say what went wrong (C-GOOD-ERR).** The crate root now re-exports
  every parsing entry point (the `parse_assembly*`, `parse_type*`,
  `parse_constant_value*` and summary-index families join the owned-module
  ones) plus the types they speak: `ParseError`, `ParseResult`, `DiagLoc`,
  `SymbolId`, `SymbolKind`, `ParsedModule`, `SlotMapping`, `GlobalRef`, and
  `ModuleSummaryIndex`. `ParseError` — the error type of the headline API —
  was previously only nameable as
  `llvmkit_asmparser::parse_error::ParseError`, and through the umbrella as
  `llvmkit::asmparser::parse_error::ParseError`. `Lexer`, `Token` and
  `Parser` stay module-scoped: they mirror LLVM's `LLLexer` / `LLParser`
  plumbing, not the surface a caller drives.
  **User-visible: the rendered error text changed.** Messages no longer
  splice a `Debug`-formatted location into the prose — `expected type at
  DiagLoc { span: Span { start: 5, end: 9 }, file: None }` is now
  `expected type`, and the location is read from `ParseError::loc()`, where
  a renderer should get it (upstream is the same shape: `LLParser::error`
  carries its `LocTy` beside the `Twine`). `SymbolKind` gains a `Display`
  and a `sigil()`, so symbol diagnostics match `LLParser.cpp` word for
  word — `redefinition of global '@foo'` and `use of undefined metadata
  '!0'` instead of `redefinition of Global '@foo'` and `use of undefined
  Metadata '%0'`; `Display for SymbolId` correspondingly prints the bare
  identity and lets the namespace supply the sigil.
  `ParseError::Expected::expected` is a `Cow<'static, str>` (nearly every
  site passes a literal, so the common diagnostic no longer allocates), and
  `Io(String)` becomes `Io { kind: std::io::ErrorKind, message: String }`,
  so `NotFound` and `PermissionDenied` are matchable without parsing the
  message back — `ErrorKind` is `Copy + Eq + Hash`, so the enum keeps its
  derives. Also: `Token` implements `Display` (replacing the allocating,
  callerless `ll_parser::describe`, same wording), `Lexer` implements
  `FusedIterator` with the `next_token`-returns-`Eof`-forever vs
  `Iterator::next`-returns-`None` contract now documented, and `Parser` has
  a manual `Debug` printing a cursor summary rather than every slot table it
  has filled.

- **Breaking (W11c): the umbrella forwards features; `llvmkit-tablegen`
  becomes a real library.** `llvmkit` gains `[features] default = ["macros"]`
  forwarding to `llvmkit-ir/macros`; the `llvmkit-ir` dependency is
  `default-features = false` there and in `llvmkit-asmparser` (which never
  used the gated re-exports), so the umbrella's feature is the one switch.
  The umbrella's three module re-exports are `#[doc(inline)]` so docs.rs
  renders the items. `llvmkit-tablegen` exposes a library API: public
  `TableGenError` (was the private `GenError`),
  `generate(llvm_root, generated_file) -> Result<Generated, TableGenError>`
  with `Generated::inputs()` listing every `.td` file read (for
  `cargo:rerun-if-changed`), `verify_generated` (the `--check` half as its
  own function), `vendored_llvm_root()`, and `GENERATED_FILE_NAME`.
  `table_gen_main()` shrinks to the CLI driver, and the empty-argv `OUT_DIR`
  build-script inference mode is gone — `llvmkit-ir`'s `build.rs` calls
  `generate` directly and replays the returned inputs as rerun lines.

- **Breaking (W12): the prose meets the tree, and the carried-forward gaps
  close.** Three small API items the parallel waves left open, plus the
  documentation reconciliation the program owed.

  `AllocaFlags` is now `pub(crate)` and no longer re-exported from the crate
  root. W10's `AllocaBuilder` toggles replaced it as the way *in*, which left
  a public type reachable from no public signature. The way *out* was the real
  gap and is now filled: `AllocaInst::is_inalloca()` and
  `AllocaInst::is_swifterror()` join `allocated_type` / `array_size` / `align`
  / `addr_space` on the read handle, mirroring `AllocaInst::isUsedWithInAlloca`
  and `isSwiftError` (`Instructions.h`) — two bool predicates, as upstream
  spells them, so the flags carrier itself stays internal plumbing.

  `Debug` for the seven builder types in `ir_builder.rs` that W11a's sweep
  could not reach: `IrBuilder`, `CallBuilder`, `TypedCallBuilder`,
  `IntrinsicCallBuilder`, `LoadBuilder`, `StoreBuilder`, `AllocaBuilder`. All
  hand-written — a `derive` would bound the folder parameter `F: Debug`, and a
  folder is a strategy type with no obligation to be printable, so the folder
  (and `TypedCallBuilder`'s lowered argument tuple) is reported by type name.
  `#[must_use]` likewise reaches the setters W11a could not: every consuming
  setter on `CallBuilder` / `TypedCallBuilder` / `IntrinsicCallBuilder`,
  `CallSiteConfig::calling_conv` / `attrs`, and
  `IrBuilder::with_fast_math_flags` / `clear_fast_math_flags`. The three memory
  builders are `#[must_use]` at the *type*, which already covers their setters
  and which `clippy::double_must_use` forbids restating.

  `IntoIterator for &NumberedValues<T>` (asmparser) completes W9's set, so
  `for (id, value) in &values` works alongside `&AttributeSet` /
  `&AttributeList` / `&MetadataAttachmentSet`; it yields exactly what
  `NumberedValues::iter` does, in the same unspecified order.

  Documentation: `AGENTS.md`'s Builder-A1 workstream entry described
  `int_load_atomic` / `load_atomic` / `store_atomic` and the
  `AtomicLoadConfig` / `AtomicStoreConfig` bags as shipped API — all five were
  deleted in W10 — and now describes the builders that replaced them, next to
  a new **builder-entry naming law** (`*_builder()` or a preposition phrase in,
  `.build()` or a typed terminal out; never `build_*`). `docs/future-work.md`'s
  "Load/store variant explosion" entry is struck as shipped, the crate READMEs
  drop `parse_assembly_string`, `BlockCursor::next`, `build_*_phi` and
  `build_{br,cond_br}_with_args`, and `UPSTREAM.md`'s row prose picks up the
  W2/W3 renames (its *test* names, as always, are frozen).

#### Removed

Everything in the bullets above that **stopped existing**, rather than being
renamed — collected here because a rename can be followed mechanically and a
deletion cannot. Each names the wave that removed it and what replaces it.

- **Breaking:** `parse_assembly_string` (W4). `parse_assembly` already accepts
  `&str` through its `impl AsRef<[u8]>` parameter.
- **Breaking:** the `&str`-taking `set_data_layout` overload, and the
  `set_data_layout_value` name beside it (W3). `set_data_layout` is now the
  single, infallible setter over a parsed `DataLayout`; parse explicitly with
  `DataLayout::parse`.
- **Breaking:** `InlineAsmOptions::with_can_unwind(bool)` and
  `SpeculationOptions::with_variable_info(bool)` (W5) — replaced by the
  zero-arg `unwind()` and `without_variable_info()`.
- **Breaking:** the atomic and volatile memory emitters `load_volatile`,
  `load_volatile_with_align`, `store_volatile`, `store_volatile_with_align`,
  `int_load_atomic`, `load_atomic`, `store_atomic` and `alloca_dyn`, together
  with the `AtomicLoadConfig` / `AtomicStoreConfig` bags they took (W10). All
  of it is reachable through `load_from` / `store_to` / `alloca_builder`.
  `AtomicCmpXchgConfig` / `AtomicRmwConfig` are **not** affected.
- **Breaking:** `AllocaFlags` is `pub(crate)` and no longer re-exported from
  the crate root (W12), which also removes its `with_inalloca` /
  `with_swifterror` setters from the public surface. Reading the flags is now
  `AllocaInst::is_inalloca()` / `is_swifterror()`; setting them is the
  `AllocaBuilder`.
- **Breaking:** `ll_parser::describe` (W11b), replaced by `Display for Token`
  with the same wording.
- `table_gen_main()`'s empty-argv `OUT_DIR` inference mode (W11c). It was a
  build-script entry point; build scripts call `generate` directly now.

### `llvmkit-tablegen`: the generator becomes a crate that mirrors LLVM

#### Added

- **`llvmkit-tablegen`**, a new workspace crate holding the TableGen front end
  and intrinsic emitter, plus the vendored `.td` tree they read. It ships an
  `llvmkit-tablegen` binary for manual regeneration and `--check`, mirroring
  `llvm-tblgen`.

#### Changed

- **The generator moved out of `llvmkit-ir`'s build script.** It was
  `crates/llvmkit-ir/tools/gen_intrinsics.rs`, a 4,197-line file pulled into
  `build.rs` with `#[path]`. `llvmkit-ir/build.rs` now calls
  `llvmkit_tablegen::table_gen_main()`, and the vendored tablegen tree moved to
  `crates/llvmkit-tablegen/tablegen/` so it stays with the generator that reads
  it.

  Two reasons. **A build script is not a test target**, so the generator's ten
  `#[test]` functions never ran under `cargo test` — despite each carrying an
  `UPSTREAM.md` provenance row implying it did. They run now. And the single
  file mirrored no upstream file, in a project whose organising rule is that it
  mirrors LLVM's: it is really two subsystems fused.

- **The crate's modules now mirror the upstream files**, on the same rule that
  makes `llvmkit-ir` mirror `llvm/lib/IR` — one module per `.cpp`, named by
  snake-casing it:

  | Module | Upstream |
  |---|---|
  | `error.rs` | `llvm/lib/TableGen/Error.cpp` |
  | `tg_lexer.rs` | `llvm/lib/TableGen/TGLexer.cpp` |
  | `tg_parser.rs` | `llvm/lib/TableGen/TGParser.cpp` |
  | `record.rs` | `llvm/lib/TableGen/Record.cpp` |
  | `lib.rs` (driver) | `llvm/lib/TableGen/Main.cpp` |
  | `main.rs` | `llvm/utils/TableGen/TableGen.cpp` |
  | `basic/code_gen_intrinsics.rs` | `llvm/utils/TableGen/Basic/CodeGenIntrinsics.cpp` |
  | `basic/intrinsic_emitter.rs` | `llvm/utils/TableGen/Basic/IntrinsicEmitter.cpp` |

  The ten tests moved to the modules they exercise, so six of them no longer
  sit in a file that does not contain the code under test. Their `UPSTREAM.md`
  rows moved with them.

  The split is pure code motion: the generated intrinsic tables are
  **byte-identical** across all 213,000 lines, before and after.

- **Breaking:** `llvmkit-ir`'s `gen-intrinsics` feature and its `gen_intrinsics`
  binary are gone. The manual entry point is the `llvmkit-tablegen` binary, whose
  usage text and `--check` message changed to match.

### VectorUtils: the intrinsic classifiers — a recorded blocker that was not real

#### Added

- **`is_trivially_vectorizable`** and **`is_trivially_scalarizable`**, porting
  the same-named functions. The first is the 71-label table that decides whether
  an intrinsic's scalar and vector forms are elementwise; the second adds the
  six `with.overflow` intrinsics on top of it.
- **`is_vector_intrinsic_with_struct_return_overload_at_field`**, porting
  `isVectorIntrinsicWithStructReturnOverloadAtField` — `frexp` is overloaded on
  both struct fields, everything else on the first only.
- **`interleave_intrinsic_factor`** and **`deinterleave_intrinsic_factor`**,
  porting `getInterleaveIntrinsicFactor` and `getDeinterleaveIntrinsicFactor`.
  Upstream returns `0` for "not one of these"; these return `Option<u32>`, so a
  factor of zero is not spellable.

These take `IntrinsicId`, which has been public all along. **Both parity
ledgers and `docs/future-work.md` recorded nine functions as blocked on
"llvmkit has no public intrinsic-id type", and that reason was wrong** —
`IntrinsicId` is public, generated-backed and spans the whole intrinsic space;
the `pub(crate)` type is `IntrinsicSemantic`, a convenience enum over a 31-name
*subset*, and the records conflated the two. All three are corrected in this
commit rather than left for the next reader to inherit.

The three VectorUtils functions still absent from that group are blocked on
things that are actually missing, now named precisely:
`isVectorIntrinsicWithScalarOpAtArg` and
`isVectorIntrinsicWithOverloadTypeAtArg` read vector-predication operand
positions out of `llvm/IR/VPIntrinsics.def`, a `.def` file llvmkit does not
vendor, and `getVectorIntrinsicIDForCall` needs `getIntrinsicForCallSite`'s
library-function half.

#### Changed

- The classifiers **drop upstream's `TargetTransformInfo` parameter**, which
  its header sanctions — passing `nullptr` "is appropriate" when no target
  specific intrinsics will be considered, and the argument gates exactly one
  branch in each. llvmkit models no target by charter, so the parameter would
  have nothing to consult. `IntrinsicId::is_target` is what a caller needs to
  tell the two apart. Each function says this at its site.

### VectorUtils: the mask predicates and constructors — the port closes

#### Added

- **The `<N x i1>` mask predicates** — `mask_is_all_zero_or_undefined`,
  `mask_is_all_one_or_undefined` and `mask_contains_all_one_or_undefined`. The
  last one's quantifier is the opposite of its name-alike's, as upstream's is.
- **The demanded-lane queries** — `possibly_demanded_elements_in_mask` and
  `horizontal_demanded_elements_for_first_operand`.
- **The mask constructors** — `create_replicated_mask`,
  `create_interleave_mask`, `create_stride_mask`, `create_sequential_mask` and
  `create_unary_mask`.
- **`masked_slide_pair`**, porting `isMaskedSlidePair`, with `MaskedSlide` and
  `ShuffleSource` replacing upstream's `(int Src, int Diff)` pairs. Upstream's
  "unused slot" marker — a `Src` of `-1` beside a `Diff` of `NumElts * 2` that
  exists only to be asserted against — is `Option`'s absence here.
- **`Constant::is_all_ones_value`**, porting `Constant::isAllOnesValue`, which
  the mask predicates need.

**`VectorUtils.cpp` is now ported as far as llvmkit's scope allows**: 20 of its
37 entry-point names are modeled (21 functions, counting both
`widenShuffleMaskElts` overloads), and the module header names what blocks each
of the other 17. *(Superseded within this same unreleased window — the
intrinsic classifiers above took five more, so the current figure is 25 of 37,
and one of the blockers recorded here turned out not to exist.)* `tests/vector_utils_parity.rs` holds that claim to account —
the modeled column is compiler-checked, so a rename cannot quietly falsify it,
and the two tables must sum to the upstream entry-point count. Two of those are permanent rather than pending —
`computeMinimumValueSizes` and `processShuffleMasks`, whose callers all live in
`lib/CodeGen/SelectionDAG` and `lib/Target/{RISCV,X86}` and which split masks
across physical registers. llvmkit models no target.

#### Changed

- `possibly_demanded_elements_in_mask` is **stronger than upstream's**, in a
  sound direction, and says so at its site: upstream reaches its element loop
  only through `dyn_cast<ConstantVector>`, which a `zeroinitializer` is not, so
  it reports "every lane demanded" for an all-zero mask that demands none.
  llvmkit stores both spellings as one element list, so the loop runs.

### VectorUtils: the shuffle-mask transforms

#### Added

- **`vector_utils::narrow_shuffle_mask_elements`,
  `widen_shuffle_mask_elements`, `widen_shuffle_mask_elements_in_pairs`,
  `scale_shuffle_mask_elements` and `shuffle_mask_with_widest_elements`** —
  rewriting a shuffle mask for a wider or narrower element type. The two
  `widenShuffleMaskElts` overloads are separate functions here rather than one
  name, because they are separate rules: the `_in_pairs` form accepts a pair
  that is half poison, which the scaled form rejects.
- Upstream's `BasicTest` fixtures for all of these are ported, plus the
  `getShuffleDemandedElts` fixture that `shuffle_demanded_elements` shipped
  without.

#### Known gaps

- The mask transforms take `&[ShuffleMaskElem]`, which spells the **IR** mask
  alphabet — a lane index or poison. Upstream's take raw `int`s and so also
  serve SelectionDAG and the X86 backend, whose alphabet extends past `-1`
  (`SM_SentinelZero` is `-2`); there, "negatives must be equal across a widened
  group" is stronger than "all poison". Code generation and target backends are
  out of scope for llvmkit, so this is a permanent narrowing rather than
  pending work, and it is unobservable on any mask llvmkit can hold. Three
  upstream assertions have no llvmkit spelling as a result; they are named in
  `tests/vector_utils_masks.rs`.

### VectorUtils: the splat family, and the vector `select` it needed

#### Added

- **`vector_utils::get_splat_value`, `splat_index`, `is_splat_value` and
  `find_scalar_element`** — the splat half of
  `llvm/lib/Analysis/VectorUtils.cpp`. `get_splat_value` names the broadcast
  scalar; `is_splat_value` answers the weaker "do all lanes agree" question and
  so sees through binary operators and `select`s that have no single scalar to
  name. Neither subsumes the other, matching upstream. Twenty-nine fixtures
  from `VectorUtilsTest.cpp` are ported verbatim, including the two upstream
  marks `FIXME` and the one it marks `TODO`.
- **`Constant::aggregate_element`**, porting `Constant::getAggregateElement`,
  and **`Constant::is_null_value`**, porting `Constant::isNullValue`. Both
  replace private partial copies that lived in `pointer_analysis`; the
  `getAggregateElement` copy handled only stored element lists, so
  `find_inserted_value` now also answers for an `undef` or `poison` aggregate,
  as upstream does.
- **`IRBuilder::build_select_erased`** — `select` where the condition may be
  `<N x i1>` and the arms may be vectors. The typed `build_select` pins the
  condition to a scalar `i1` and the arms to a `SelectArm` marker, neither of
  which a vector select can satisfy; this is the same erased split
  `build_int_binop_erased` and `build_int_cmp_erased` already make. Validated
  against `SelectInst::areInvalidOperands`.

#### Fixed

- **The `.ll` parser rejected `select` over aggregate arms.** It restricted
  arms to int/fp/ptr and said so in its own diagnostic ("select arm category
  supported by this parser"). `LLParser::parseSelect` delegates wholly to
  `SelectInst::areInvalidOperands`, which names no arm restriction beyond
  token, so `select i1 %c, { i32, i32 } %a, { i32, i32 } %b` is valid LLVM.
  Only the token check remains, and it stays *before* constant folding —
  otherwise two equal token arms would fold away instead of being rejected.
- **The `.ll` parser rejected every vector `select`.** It validated a
  `<N x i1>` condition as legal and then unconditionally narrowed the condition
  to a scalar handle, so `select <2 x i1> %c, <2 x i8> %t, <2 x i8> %f` — plain
  LLVM that `clang` emits — failed to parse. A scalar `i1` condition over
  vector arms failed too, for the same reason one level down. Both parse,
  verify and print byte-identically now. The arm-category restriction the
  parser announces (int/fp/ptr, scalar or vector) is unchanged: `select` over a
  struct or array arm is valid LLVM that it still declines.

#### Changed

- `value_tracking`'s private `shuffle_splat_source` moved to
  `vector_utils::get_splat_value` and gained the constant-vector branch it
  lacked. Its one caller inside `isGuaranteedNotToBeUndefOrPoison` now spells
  upstream's `isa<ShuffleVectorInst>(Opr) ? getSplatValue(Opr) : nullptr`
  directly.

#### Fixed (analysis precision)

- **Both `shufflevector` analysis arms now take upstream's `getSplatValue`
  fast path**, which `case Instruction::ShuffleVector:` opens with in
  `computeKnownBits` and in `computeKnownFPClass` alike. The case it buys is a
  splat mask carrying a poison lane — `<0, poison, 0, 0>`, which `m_ZeroMask`
  accepts: the demanded-lane path gives up there, because
  `getShuffleDemandedElts` rejects a demanded poison element, while the splat
  match reads straight through to the scalar. Known bits and the float class of
  such a broadcast now match the scalar being broadcast instead of answering
  "nothing known".

#### Also fixed, in follow-up

- **`Constant::splat_value` gained upstream's constant-*expression* arm** — the
  `shufflevector`-of-`insertelement` shape `ConstantVector::getSplat` builds.
  It only ever fires for a *scalable* vector: llvmkit's folder materialises a
  fixed one into an element list at construction, so the element-list arm
  answers those first. The two mask representations turned out not to need
  reconciling — `ConstantExprData`'s `mask` field is vestigial, since
  `validate_constant_expr_data` rejects a non-empty one for `ShuffleVector` and
  all 114 construction sites pass empty. The mask is the third operand, and
  that is what the arm reads.
#### Fixed, in follow-up

- **A scalable vector of uniform pointers or `undef` printed an element list**,
  which is not a constant form LLVM has for a scalable type — its lane count is
  a minimum, so a list cannot describe the lanes one for one, and llvmkit was
  also implicitly asserting `vscale == 1`. `asm_writer::prints_as_splat` now
  answers the two vector kinds separately: a **fixed** vector keeps
  `AsmWriter.cpp`'s `isa<ConstantInt> || isa<ConstantFP>` restriction on the
  `splat (…)` shorthand, since its element list is an equally legal spelling;
  a **scalable** vector uses `splat (…)` for any uniform element, because that
  is the only spelling it has. Output for every constant LLVM can also build is
  unchanged.

  The cause was in the printer, not the folder — an earlier attempt to guard
  `constant_fold::vector_splat_constant` broke five tests that depend on
  llvmkit representing a scalable splat as a uniform element list, and was
  reverted.

#### Known gaps

- **A non-uniform scalable aggregate still prints an element list**, which LLVM
  would still reject. That constant should not be constructible at all:
  `VectorType::const_vector` skips its element-count check for scalable types
  (correctly — the count means nothing there) and does not require the lanes to
  agree. Requiring it was tried and reverted, because two tests build such a
  constant as their *premise* to check the folder declines, so it is a
  representation-policy decision rather than a patch. See
  `docs/future-work.md`.
### Vector floating-point operations

#### Added

- **`IRBuilder::build_fp_binop_erased`, `build_fp_cmp_erased` and
  `build_fp_neg_erased`** — the floating-point half of the erased builder
  family, beside the integer `build_int_binop_erased` / `build_int_cmp_erased`
  and the `build_select_erased` added earlier this cycle. They exist for one
  reason: llvmkit's typed float handles carry a *scalar* `FloatKind`, so
  `<N x double>` has no typed handle to route through `IntoFloatValue`.
  Upstream needs no split — `LLParser::parseArithmetic` and `parseCompare` hand
  their operands to `BinaryOperator::Create` and `CmpInst::Create`.

#### Fixed

- **The parser could not read any vector floating-point operation.**
  `fadd <4 x float> %a, %b` answered "float-typed lhs", and so did `fcmp` and
  `fneg` — all three narrowed operands through the scalar-only path. Fast-math
  flags are carried through unchanged; `fcmp` is an `FPMathOperator` upstream,
  so it keeps its flags too. Scalar operators are unaffected.

### ValueTracking: `computeKnownFPClass`'s vector arms

#### Added

- `extractelement`, `insertelement`, `shufflevector`, `extractvalue`,
  `bitcast` and `phi`, plus the `fma`/`fmuladd` pair and the four reducing
  min/max intrinsics (`vector.reduce.fmax`/`fmin`/`fmaximum`/`fminimum`).
  Every one of these previously answered `fcAllFlags`.
- `KnownFpClass::reset_sign_bit`, porting `KnownFPClass::SignBit.reset()`. The
  reducing min/max arm needs it: those may return a NaN whose sign is not the
  one the elements agreed on, so a sign learned from the elements is dropped
  unless the result is known never to be a NaN.

#### Changed

- `getShuffleDemandedElts` is now a shared `shuffle_demanded_elements` helper
  rather than logic inlined in `shuffle_vector_known_bits`. The poison-lane
  rule is subtle enough that two copies would eventually disagree.

Two arms diverge from upstream, in opposite directions, marked at their sites
and in the module header:

- `shufflevector` is **weaker** in exactly one case. `getSplatValue`'s
  `m_ZeroMask` accepts `-1` alongside `0`, so upstream reads
  `<0, poison, 0, 0>` as a splat and answers from the scalar; the demanded-lane
  path here sees a demanded poison lane and gives up. A clean all-zero mask
  reaches the same answer either way.
- `bitcast` can be **stronger**. Upstream calls `computeKnownBits` at
  `Depth + 1` on the shared budget, so a bitcast reached late in an FP walk
  learns nothing; llvmkit's known bits starts at zero and gets a full budget.
  Sound, but a divergence upward, and it costs compile time.

### ValueTracking: `computeKnownFPClass`'s arithmetic arms, and `nofpclass`

#### Added

- `nofpclass(nan inf)` parses, prints and round-trips on parameters and
  returns, and is off `attribute_td_drift.rs`'s not-yet-modeled list. The
  payload is the `FpClassTest` it means rather than upstream's raw integer,
  following `Attribute::Memory(MemoryEffects)`.
- `computeKnownFPClass` reads it: a call's return `nofpclass` and an argument's
  parameter `nofpclass` now open the ruled-out set, as
  `CallBase::getRetNoFPClass` and `Argument::getNoFPClass` do upstream.
- The arithmetic arms — `fadd`, `fsub`, `fmul`, `fdiv` and `frem`. Previously
  every one answered `fcAllFlags`. This includes the special cases upstream
  carries: `fadd x, x` as the canonical `fmul x, 2`, `x * x` through
  `KnownFPClass::square`, the denormal-mode zero refinements, `x / x` as
  exactly `1.0` or NaN, and `x % x` as exactly `±0.0` or NaN.

Tests are `ComputeKnownFPClassTest`'s `FAdd`, `FSub`, `FMul` and `FMulNoZero`,
ported verbatim — which is only possible because `nofpclass` is modeled, since
upstream builds every operand in them out of it. **`fdiv` and `frem` have no
upstream unit test**; that is recorded in `known_fp_class.rs`'s header rather
than papered over with an invented fixture.

### Parser / IR: integer casts over vectors

#### Added

- `trunc` / `zext` / `sext` over fixed and scalable integer vectors now parse,
  verify, and print. Previously only the scalar spellings worked:
  `zext <4 x i1> %c to <4 x i32>` failed with "integer-typed cast source".
- `IRBuilder::build_int_cast_erased`, the third member of the erased builder
  family alongside `build_int_binop_erased` and `build_int_cmp_erased`. llvmkit's
  typed integer handles carry a *scalar* width, so `<N x iM>` converts to none of
  them; the erased path is how the other vector instructions were already built.
- `IntCastFlags`, the runtime-opcode counterpart of `IntBinOpFlags`: a caller
  holding a `CastOpcode` it does not know statically cannot choose between
  `TruncFlags` and `ZExtFlags`, so it supplies all three flags and the builder
  writes through whichever the opcode reads.

#### Fixed

- The verifier rejected every vector integer cast. Its `trunc`/`zext`/`sext`
  arm read operand widths through an integer-only accessor, so a vector cast
  failed with "source type `<4 x i1>` is not integer" even once it parsed. It
  now compares `Type::getScalarSizeInBits` and checks the vector shapes
  separately, which is what `CastInst::castIsValid` specifies: both sides
  vectors of equal element count and scalability, or both scalars, with a
  strict width change in the opcode's direction.

### ValueTracking: two of the residue — `stripNullTest` and `collectPossibleValues`

#### Added

- `strip_null_test`, porting `llvm::stripNullTest`: given the ceiling-division
  idiom `(X >> C) or/add zext(X & mask(C) != 0)`, it recovers `X`. The shift
  carries every bit at or above `C` and the compare folds the bits below it
  into one flag, so the whole expression is zero exactly when `X` is.
- `collect_possible_values`, porting `llvm::collectPossibleValues`: the
  constants a value can take, walking back through `select` and `phi`.
  Upstream fills a caller-owned set and returns whether the enumeration is
  complete; here the two are one `Option`, because an incomplete set is
  precisely what a caller must not act on.

#### Fixed

- `is_known_non_zero` now retries through `strip_null_test`, which is the tail
  of upstream's `isKnownNonZero`. It answers `true` on the ceiling-division
  idiom whenever it can answer `true` on the operand — previously it fell
  through to known bits, which prove nothing there. Its doc comment now also
  says plainly that the rest of that function is a known-bits approximation of
  upstream's dedicated walk, not a line-for-line port.

Ledger: **93 of 101 modeled, 8 gaps.**

### ValueTracking: `getInverseMinMaxIntrinsic` across both halves of the family

#### Added

- `MinMaxOperation` — a min/max intrinsic, integer or floating-point. Upstream
  spells this as an `Intrinsic::ID`, one flat type naming every intrinsic there
  is, and narrows it with a `switch` whose `default` is `llvm_unreachable`.
  Here it is the sum of two closed enums that already existed for their own
  reasons: `MinMaxIntrinsic` (the four integer intrinsics, exactly the range of
  `getMinMaxIntrinsic`) and `MinMaxKind` (the six floating-point ones, porting
  `KnownFPClass::MinMaxKind`). The arms are disjoint and together are exactly
  upstream's ten, so every mapping over the sum is total.
- `MinMaxOperation::inverse`, porting `llvm::getInverseMinMaxIntrinsic` over
  its whole domain, and `MinMaxKind::inverse` for the six floating-point arms
  it delegates to. `MinMaxIntrinsic::inverse`, the integer half, already
  existed. Also `MinMaxKind::name` and `MinMaxOperation::name`, matching the
  `MinMaxIntrinsic::name` that was already there.

#### Fixed

- **Breaking:** `can_convert_to_min_or_max_intrinsic` returns
  `Option<(MinMaxOperation, bool)>` rather than `Option<(MinMaxIntrinsic,
  bool)>`. It used to answer `None` for the two floating-point flavours, where
  upstream's switch answers `Intrinsic::maxnum` / `Intrinsic::minnum`; a caller
  was told "cannot convert" about a `select` that converts fine. Both arms now
  answer.

  The narrowing, the ledger's gap reason, and `MinMaxIntrinsic::inverse`'s doc
  comment all rested on one claim — "llvmkit models no floating-point min/max
  intrinsic" — that was true when tranche 4b wrote it and false from tranche 7a
  onward, once `MinMaxKind` landed with exactly the six variants upstream
  inverts. All three are corrected.

Ledger: **91 of 101 modeled, 10 gaps.**

#### Internal

- The parity ledger's `ValueTracking` half now proves what its module header
  already claimed. `every_modeled_known_bits_row_is_exercised` has read the
  test file's own source since 2026-08-03 to check that every modeled
  `KnownBits` row is reached by real code; the `ValueTracking` table had no
  such tie, and had drifted — `getInverseMinMaxFlavor`, `getMinMaxIntrinsic`,
  `getMinMaxLimit` and `getMinMaxPred` sat in the modeled column with nothing
  naming them, and `getSelectPattern`, `SelectPatternResult` and
  `SelectPatternNaNBehavior` turned out the same way once the new
  `every_modeled_value_tracking_row_is_exercised` went looking. All seven are
  exercised now. No API change; this is the ledger holding itself to its own
  claim.

### ValueTracking: the and/xor/or known-bits idioms

#### Added

- `analyze_known_bits_from_and_xor_or`, porting
  `llvm::analyzeKnownBitsFromAndXorOr`: the known bits of an `and` / `or` /
  `xor` given both operands' bits, which upstream exposes so
  `SimplifyDemandedUseBits` can reuse the reasoning with bits it has already
  narrowed. `None` for a value that is not one of the three — upstream reaches
  an `llvm_unreachable` there, which is a caller precondition.

#### Fixed

- The `and` / `or` / `xor` known-bits walk was missing two of upstream's
  refinements, so it answered weaker than LLVM on both: `and(x, -x)` isolates
  the lowest set bit (`KnownBits::blsi`, taken from whichever operand has the
  fewer possible trailing zeros) and `xor(x, x - 1)` masks the low bits
  (`KnownBits::blsmsk`). Both need a bit already known set, which is upstream's
  own `HasKnownOne` gate. The odd-operand refinement that was already there is
  unchanged.

Ledger: **90 of 101 modeled, 11 gaps.**

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

> Superseded on that last one: the gap was not real. `can_convert_to_min_or_max_intrinsic`
> now answers for both flavours — see *`getInverseMinMaxIntrinsic` across both
> halves of the family* at the top of this file.

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

> Superseded: `MinMaxKind` (tranche 7a) turned out to name exactly those six,
> and the row closed in full — see *`getInverseMinMaxIntrinsic` across both
> halves of the family* at the top of this file.


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
