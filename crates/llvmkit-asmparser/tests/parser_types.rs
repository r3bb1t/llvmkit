//! Type-definition parsing: identity, forward references, and aliases.
//!
//! Mirrors `LLParser::parseStructDefinition` and the `%name` / `%N` arms of
//! `LLParser::parseType`. Citations live in `UPSTREAM.md`.

use llvmkit_asmparser::{ll_parser::Parser, parse_error::ParseError};
use llvmkit_ir::Module;

fn parse_render(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect("parse succeeded");
    format!("{module}")
}

fn parse_err(src: &[u8]) -> ParseError {
    let module = Module::dynamic("parser_types_err");
    Parser::new(src, &module)
        .expect("parse constructor")
        .parse_module()
        .expect_err("parse rejected")
}

/// Two numbered types with identical bodies are *distinct* types, because
/// `%N = type ...` creates an identified struct (`StructType::create`), not a
/// literal one (`StructType::get`). llvmkit minted a literal struct for each,
/// so structural uniquing silently merged them.
///
/// No upstream `.ll` isolates this; the rule is the `StructType::create` call
/// in `LLParser::parseStructDefinition`, and the observable consequence is
/// that both definitions survive the round-trip.
#[test]
fn numbered_types_with_equal_bodies_stay_distinct() {
    let text = parse_render(
        "numbered_type_identity",
        b"%0 = type { i32 }\n%1 = type { i32 }\n@g = global %1 zeroinitializer\n",
    );
    assert!(text.contains("%0 = type { i32 }"), "{text}");
    assert!(text.contains("%1 = type { i32 }"), "{text}");
    assert!(text.contains("@g = global %1 zeroinitializer"), "{text}");
}

/// A numbered type may be defined at any slot: upstream's `NumberedTypes` is
/// a plain map with no monotonicity rule. llvmkit required each definition to
/// equal the running frontier and rejected `%5` as the first one.
///
/// No upstream `.ll` isolates this; the rule is the absence of any check in
/// `LLParser::parseStructDefinition`'s numbered path.
#[test]
fn a_numbered_type_may_skip_slots() {
    let text = parse_render("numbered_type_skip", b"%5 = type { i32 }\n");
    assert!(text.contains("type { i32 }"), "{text}");
}

/// `%t = type opaque` is a definition even though it leaves the body unset,
/// so a later `%t = type {i32}` is a redefinition. Mirrors the
/// `Entry.first && !Entry.second.isValid()` guard at the top of
/// `LLParser::parseStructDefinition`, whose wording this pins.
#[test]
fn redefining_a_type_after_opaque_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type opaque\n%t = type { i32 }\n").to_string(),
        "redefinition of type"
    );
}

/// The same guard for two bodied definitions.
#[test]
fn redefining_a_bodied_type_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type { i32 }\n%t = type { i64 }\n").to_string(),
        "redefinition of type"
    );
}

/// A `type` directive whose right-hand side is not a struct is a plain alias,
/// accepted "for compatibility with old files" — and, because there is no
/// identified struct to fill in later, it may not have been forward
/// referenced. Mirrors the `if (Entry.first) return error(...)` arm of
/// `LLParser::parseStructDefinition`.
#[test]
fn a_forward_referenced_alias_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%t)\n%t = type i32\n").to_string(),
        "forward references to non-struct type"
    );
}

/// The same alias without a prior reference is legal, and the alias resolves
/// to the aliased type rather than to a struct.
#[test]
fn a_type_alias_resolves_to_the_aliased_type() {
    let text = parse_render(
        "type_alias",
        b"%t = type i32\n@g = global %t zeroinitializer\n",
    );
    assert!(text.contains("@g = global i32 0"), "{text}");
}

/// A numbered type referenced but never defined. Mirrors the `NumberedTypes`
/// loop in `LLParser::validateEndOfModule`; llvmkit used to leave the
/// reference as a silently opaque struct, with a comment asserting that
/// upstream does not diagnose this.
#[test]
fn an_undefined_numbered_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%3)\n").to_string(),
        "use of undefined type '%3'"
    );
}

/// The named twin, whose noun differs: upstream says `type named 'x'` where
/// the numbered form says `type '%N'`.
#[test]
fn an_undefined_named_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%t)\n").to_string(),
        "use of undefined type named 't'"
    );
}

/// A forward-referenced numbered type resolves to the *same* type its later
/// definition fills in — the property an anonymous identified struct exists
/// for. A literal struct could not do this: the forward reference and the
/// definition would be two different types.
#[test]
fn a_forward_referenced_numbered_type_resolves_to_its_definition() {
    let text = parse_render(
        "numbered_type_forward",
        b"@g = global ptr null\n%0 = type { i32 }\n@h = global %0 zeroinitializer\n",
    );
    assert!(text.contains("%0 = type { i32 }"), "{text}");
    assert!(text.contains("@h = global %0 zeroinitializer"), "{text}");
}

// ── Element / shape validity (`Type.cpp`'s isValidElementType family) ──────

/// `zero element vector is illegal` and `size too large for vector` — the two
/// shape checks in `LLParser::parseArrayVectorType`. Upstream reads the count
/// as an `APSInt` and range-checks it *after*, which is why an over-large
/// count is this message rather than a parse failure.
#[test]
fn vector_shape_is_checked() {
    assert_eq!(
        parse_err(b"declare void @f(<0 x i32>)\n").to_string(),
        "zero element vector is illegal"
    );
    assert_eq!(
        parse_err(b"declare void @f(<4294967296 x i32>)\n").to_string(),
        "size too large for vector"
    );
}

/// `VectorType::isValidElementType` is the one *allow*-list in the family:
/// integers, floats, pointers, and target extension types that declare
/// `CanBeVectorElement`. A struct element is not in it.
#[test]
fn invalid_vector_element_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(<2 x {i32}>)\n").to_string(),
        "invalid vector element type"
    );
}

/// `ArrayType::isValidElementType` is a deny-list, and denies `x86_amx` where
/// the struct predicate does not.
#[test]
fn invalid_array_element_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f([2 x label])\n").to_string(),
        "invalid array element type"
    );
    assert_eq!(
        parse_err(b"declare void @f([2 x x86_amx])\n").to_string(),
        "invalid array element type"
    );
}

/// `StructType::isValidElementType`, checked per element against that
/// element's own location (`LLParser::parseStructBody`). `x86_amx` is legal
/// here — the difference from the array predicate is deliberate upstream.
#[test]
fn invalid_struct_element_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f({i32, label})\n").to_string(),
        "invalid element type for struct"
    );
    let text = parse_render("struct_amx_element", b"declare void @f({i32, x86_amx})\n");
    assert!(text.contains("x86_amx"), "{text}");
}

/// `ptr*` is rejected where a pointee-typed pointer would have been read.
/// Mirrors the check `LLParser::parseType` makes immediately after building
/// the opaque pointer, before its suffix loop runs.
#[test]
fn ptr_star_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(ptr*)\n").to_string(),
        "ptr* is invalid - use ptr instead"
    );
}

/// `FunctionType::isValidReturnType` via `LLParser::parseFunctionType`.
///
/// Written as a type alias rather than `label ()*`: the legacy typed-pointer
/// spelling goes through llvmkit's lookahead, which skips the pointee
/// syntactically and never builds the function type at all.
#[test]
fn invalid_function_return_type_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type label ()\n").to_string(),
        "invalid function return type"
    );
}

/// `FunctionType::isValidArgumentType` via `LLParser::parseArgumentList` —
/// first-class and not `label`.
#[test]
fn invalid_function_argument_type_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type void (label)\n").to_string(),
        "invalid type for function argument"
    );
}

/// Upstream shares `parseArgumentList` between a function *type* and a
/// function *header*, so a name and attributes parse in type position and are
/// rejected afterwards — which is the only reason these two messages exist.
/// llvmkit read bare types there, so both were unreachable behind a generic
/// `expected ')'`.
#[test]
fn a_name_or_attribute_in_a_function_type_is_rejected() {
    assert_eq!(
        parse_err(b"%t = type void (i32 %x)\n").to_string(),
        "argument name invalid in function type"
    );
    assert_eq!(
        parse_err(b"%t = type void (i32 nocapture)\n").to_string(),
        "argument attributes invalid in function type"
    );
}

// ── Legacy typed-pointer suffixes (`LLParser::parseType`'s suffix loop) ────

/// The three pointee rejections. All were unreachable: llvmkit's lookahead
/// skipped the pointee type syntactically and lowered straight to opaque
/// `ptr`, so there was never a pointee to ask about.
#[test]
fn invalid_pointee_types_are_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(label*)\n").to_string(),
        "basic block pointers are invalid"
    );
    assert_eq!(
        parse_err(b"declare void @f(void*)\n").to_string(),
        "pointers to void are invalid - use i8* instead"
    );
    assert_eq!(
        parse_err(b"declare void @f(metadata*)\n").to_string(),
        "pointer to this type is invalid"
    );
}

/// The `addrspace` arm words the `void` rejection with a semicolon where the
/// `*` arm uses a dash. That is upstream's own inconsistency, and diagnostic
/// text is contractual, so it is reproduced rather than smoothed.
#[test]
fn the_addrspace_arm_words_the_void_rejection_differently() {
    assert_eq!(
        parse_err(b"declare void @f(void addrspace(1)*)\n").to_string(),
        "pointers to void are invalid; use i8* instead"
    );
    assert_eq!(
        parse_err(b"declare void @f(label addrspace(1)*)\n").to_string(),
        "basic block pointers are invalid"
    );
}

/// A legacy typed pointer still lowers to an opaque pointer once its pointee
/// has been checked — the pointee type is parsed, not represented.
#[test]
fn a_valid_typed_pointer_lowers_to_an_opaque_pointer() {
    let text = parse_render(
        "typed_pointer_lowering",
        b"declare void @f(i32*, i8 addrspace(3)*)\n",
    );
    assert!(
        text.contains("declare void @f(ptr %0, ptr addrspace(3) %1)"),
        "{text}"
    );
}

/// Now that the atom is parsed for real, `%t*` looks `%t` up — so an
/// undefined one is caught by `validateEndOfModule` rather than silently
/// lowered away.
#[test]
fn a_pointer_to_an_undefined_named_type_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(%t*)\n").to_string(),
        "use of undefined type named 't'"
    );
}

// ── Target extension types (`LLParser::parseTargetExtType`) ───────────────

/// Type parameters must precede integer ones; once an integer has been seen,
/// anything else is `expected uint32 param`. llvmkit reported a generic
/// `expected target extension type` here.
#[test]
fn a_type_param_after_an_int_param_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(target(\"spirv.Type\", 1, i32))\n").to_string(),
        "expected uint32 param"
    );
}

/// `TargetExtType::checkParams` (`llvm/lib/IR/Type.cpp`) — the three named
/// types that constrain their own arity. Upstream reaches these through
/// `getOrError`; llvmkit runs the same check from the parser, the only place
/// a malformed one can be written.
#[test]
fn target_extension_type_arity_is_checked() {
    assert_eq!(
        parse_err(b"declare void @f(target(\"aarch64.svcount\", i32))\n").to_string(),
        "target extension type aarch64.svcount should have no parameters"
    );
    assert_eq!(
        parse_err(b"declare void @f(target(\"riscv.vector.tuple\", i32))\n").to_string(),
        "target extension type riscv.vector.tuple should have one type parameter and one integer parameter"
    );
    assert_eq!(
        parse_err(b"declare void @f(target(\"amdgcn.named.barrier\"))\n").to_string(),
        "target extension type amdgcn.named.barrier should have no type parameters and one integer parameter"
    );
}

/// A target extension type may be parameterised by `void`
/// (`parseType(TypeParam, /*AllowVoid=*/true)`), which llvmkit rejected.
#[test]
fn a_target_extension_type_may_take_a_void_parameter() {
    let text = parse_render(
        "target_ext_void_param",
        b"declare void @f(target(\"t\", void))\n",
    );
    assert!(text.contains("target(\"t\", void)"), "{text}");
}

// ── Symbolic address spaces (`LLParser::parseOptionalAddrSpace`) ──────────

/// `addrspace("A"|"G"|"P")` resolve through the module's data layout —
/// alloca, default-globals and program address spaces respectively. Mirrors
/// the `ParseAddrspaceValue` lambda in `LLParser::parseOptionalAddrSpace`.
#[test]
fn symbolic_address_spaces_resolve_through_the_data_layout() {
    let text = parse_render(
        "symbolic_addrspace",
        b"target datalayout = \"A5-G2-P3\"\n@g = external global i32, align 4\ndeclare void @f(ptr addrspace(\"A\"), ptr addrspace(\"G\"), ptr addrspace(\"P\"))\n",
    );
    assert!(
        text.contains("ptr addrspace(5)") && text.contains("ptr addrspace(2)"),
        "{text}"
    );
    assert!(text.contains("ptr addrspace(3)"), "{text}");
}

/// An unknown symbolic spelling is `invalid symbolic addrspace 'X'`.
#[test]
fn an_unknown_symbolic_address_space_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(ptr addrspace(\"Q\"))\n").to_string(),
        "invalid symbolic addrspace 'Q'"
    );
}

/// Anything that is neither a number nor a string is
/// `expected integer or string constant`.
#[test]
fn a_non_constant_address_space_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(ptr addrspace(i32))\n").to_string(),
        "expected integer or string constant"
    );
}

/// `isUInt<24>` — upstream range-checks the parsed value, so the diagnostic
/// is about the number rather than the token.
#[test]
fn an_address_space_wider_than_24_bits_is_rejected() {
    assert_eq!(
        parse_err(b"declare void @f(ptr addrspace(16777216))\n").to_string(),
        "invalid address space, must be a 24-bit integer"
    );
}

// ── Constant type agreement (`LLParser::convertValIDToValue`) ─────────────

/// The `ValID::t_Constant` arm of `LLParser::convertValIDToValue`: a parsed
/// constant carries its own type, and nothing before this point checks it
/// against the type the context asked for.
///
/// Ports the negative half of `test/Bitcode/blockaddress-addrspace.ll`
/// (`return-self-bad.ll`), whose CHECK line pins this message. The function
/// is `addrspace(1)`, so its `blockaddress` is `ptr addrspace(1)` while the
/// `ret` wants `ptr addrspace(2)` — the address space is what disagrees,
/// which is why the fixture exists.
#[test]
fn a_constant_of_the_wrong_type_is_rejected() {
    let module = Module::dynamic("constant_type_mismatch");
    let err = Parser::new(
        include_bytes!("fixtures/upstream/blockaddress-addrspace/return_self_bad.ll"),
        &module,
    )
    .expect("parse constructor")
    .parse_module()
    .expect_err("the fixture is a negative test");
    assert_eq!(
        err.to_string(),
        "constant expression type mismatch: got type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'"
    );
}

/// Ports `test/Assembler/invalid_cast4.ll`, whose CHECK pins
/// `CastInst::castIsValid` reached from a *constant expression*:
/// `inttoptr (i64 0 to i64)` names a destination that is not a pointer.
///
/// llvmkit asked a different question here — whether the destination matched
/// the initializer's type — which upstream does not ask at all. That
/// agreement is `convertValIDToValue`'s job (W4 part 1).
#[test]
fn an_invalid_constexpr_cast_opcode_is_rejected() {
    let module = Module::dynamic("invalid_cast4");
    let err = Parser::new(
        include_bytes!("fixtures/upstream/invalid-cast/invalid_cast4.ll"),
        &module,
    )
    .expect("parse constructor")
    .parse_module()
    .expect_err("the fixture is a negative test");
    assert_eq!(
        err.to_string(),
        "invalid cast opcode for cast from 'i64' to 'i64'"
    );
}

// ── convertValIDToValue message family ────────────────────────────────────

/// The guard at the very top of `LLParser::convertValIDToValue`, before any
/// `ValID` arm runs: a function *type* in value position is always this
/// error, whatever the value turns out to be.
#[test]
fn a_function_typed_value_is_rejected() {
    assert_eq!(
        parse_err(b"@g = global void () zeroinitializer\n").to_string(),
        "functions are not values, refer to them as pointers"
    );
}

/// `integer constant must have integer type` and `floating point constant
/// invalid for type` — the `t_APSInt` and `t_APFloat` arms. llvmkit worded
/// both as `expected <production>`.
#[test]
fn a_constant_of_the_wrong_literal_kind_is_rejected() {
    assert_eq!(
        parse_err(b"@g = global float 3\n").to_string(),
        "integer constant must have integer type"
    );
    assert_eq!(
        parse_err(b"@g = global i32 3.0\n").to_string(),
        "floating point constant invalid for type"
    );
}

/// `null must be a pointer type` — the `t_Null` arm.
#[test]
fn null_at_a_non_pointer_type_is_rejected() {
    assert_eq!(
        parse_err(b"@g = global i32 null\n").to_string(),
        "null must be a pointer type"
    );
}

/// The `t_Undef` / `t_Poison` / `t_Zero` arms share a first-class guard that
/// llvmkit had no equivalent of. An *opaque* identified struct is the
/// reachable half — a struct with no body, so `isFirstClassType` is false.
/// (The `label` half needs a value position at label type, which the grammar
/// does not offer; upstream carries a `FIXME` about `label` being
/// first-class at all.)
#[test]
fn undef_like_constants_reject_a_non_first_class_type() {
    assert_eq!(
        parse_err(b"%t = type opaque\n@g = global %t undef\n").to_string(),
        "invalid type for undef constant"
    );
    assert_eq!(
        parse_err(b"%t = type opaque\n@g = global %t poison\n").to_string(),
        "invalid type for poison constant"
    );
}
