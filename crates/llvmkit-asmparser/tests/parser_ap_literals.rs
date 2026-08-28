//! Parser coverage for APInt/APFloat-backed numeric literals.
//!
//! Ports `LLLexer.cpp` numeric token forms and `LLParser.cpp::parseValID`
//! conversion through typed constants.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_render(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

fn parse_err(src: &[u8]) -> String {
    let module = Module::dynamic("parser_ap_literals_error");
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("literal is rejected")
        .to_string()
}

/// Port of `LLParser.cpp::parseValID` decimal APSInt conversion for arbitrary width.
#[test]
fn decimal_i129_literal_round_trips_without_host_truncation() {
    let text = parse_and_render(
        "decimal_i129_literal_round_trips_without_host_truncation",
        b"@g = global i129 340282366920938463463374607431768211456\n",
    );
    assert!(
        text.contains("@g = global i129 -340282366920938463463374607431768211456"),
        "{text}"
    );
}

/// Port of `LLLexer.cpp` `u0x` APSInt token and `LLParser.cpp::parseValID` typed lowering.
#[test]
fn unsigned_hex_i129_literal_round_trips() {
    let text = parse_and_render(
        "unsigned_hex_i129_literal_round_trips",
        b"@g = global i129 u0x100000000000000000000000000000000\n",
    );
    assert!(
        text.contains("@g = global i129 -340282366920938463463374607431768211456"),
        "{text}"
    );
}

/// An integer literal token carries the width the *token* needs, never the
/// one its context wants, and two upstream behaviours follow from that.
///
/// `LLLexer::lexIdentifier`'s `[us]0x` tail builds the value at `4 * digits`
/// bits and truncates it to its **active** bits before the `s` / `u` prefix
/// decides the signedness — so `s0x0F` is a four-bit signed `0b1111`, which
/// is −1, not 15. Reading it at the destination width instead gives `+15`
/// with no diagnostic, which is a wrong value rather than a missing error.
///
/// `convertValIDToValue`'s `t_APSInt` arm then applies `extOrTrunc`, not a
/// checked widening, so `i8 300` is `44` and is accepted. llvmkit used to
/// build the literal straight at `i8` and refuse it as an overflow.
///
/// No upstream `.ll` pins either: `test/Assembler/invalid-hexint.ll` is the
/// tree's only `[us]0x` fixture and it turns on a *malformed* token, so it
/// says nothing about a well-formed one's width. (It is ported, in
/// `parser_diagnostics.rs::a_malformed_hex_apsint_matches_upstream_text`.)
/// Anchored by symbol instead.
#[test]
fn integer_literals_carry_the_lexers_own_width() {
    let text = parse_and_render(
        "integer_literals_carry_the_lexers_own_width",
        b"@n = global i64 s0x0F\n@w = global i8 300\n",
    );
    assert!(text.contains("@n = global i64 -1"), "{text}");
    assert!(text.contains("@w = global i8 44"), "{text}");
}

/// Port of `LLLexer.cpp` signed decimal literals through `LLParser.cpp::parseValID`.
#[test]
fn negative_wide_decimal_literal_round_trips_as_signed_bits() {
    let text = parse_and_render(
        "negative_wide_decimal_literal_round_trips_as_signed_bits",
        b"@g = global i129 -1\n",
    );
    assert!(text.contains("@g = global i129 -1"), "{text}");
}

/// Port of `LLParser.cpp::parseValID` decimal APFloat conversion.
#[test]
fn decimal_double_literal_round_trips_through_apfloat() {
    let text = parse_and_render(
        "decimal_double_literal_round_trips_through_apfloat",
        b"@g = global double 1.000000e+00\n",
    );
    assert!(text.contains("@g = global double 1.000000e+00"), "{text}");
}

/// `LLLexer::LexDigitOrNegative` builds **every** decimal floating literal at
/// `IEEEdouble` — it has no type information — and `convertValIDToValue`'s
/// `t_APFloat` arm narrows afterwards, rejecting via
/// `ConstantFP::isValueValidForType` when the narrowing would lose anything.
///
/// So a decimal `half` literal is legal exactly when it survives the double
/// round-trip: `1 + 2^-10` is a half value and passes, while `1 + 2^-11` is
/// halfway between two half values and does not.
///
/// These two used to assert the opposite — that llvmkit parses the decimal
/// *directly* at half semantics, which skips LLParser's double round-trip and
/// gives a different answer: read at half, `1 + 2^-11` plus a trailing digit
/// rounds **up** to `0xH3C01`; read as a double first, it ties to even and
/// rounds **down**, then loses information and is refused.
#[test]
fn decimal_half_literal_goes_through_double() {
    let text = parse_and_render(
        "decimal_half_literal_goes_through_double",
        b"@h = global half 1.0009765625\n",
    );
    assert!(text.contains("@h = global half 0xH3C01"), "{text}");

    assert_eq!(
        parse_err(b"@h = global half 1.0004882812500000000000000000000001\n"),
        "floating point constant invalid for type"
    );
}

/// The `bfloat` twin of the test above: seven significand bits, so `1 + 2^-7`
/// passes and `1 + 2^-8` is the tie that does not.
#[test]
fn decimal_bfloat_literal_goes_through_double() {
    let text = parse_and_render(
        "decimal_bfloat_literal_goes_through_double",
        b"@b = global bfloat 1.0078125\n",
    );
    assert!(text.contains("@b = global bfloat 0xR3F81"), "{text}");

    assert_eq!(
        parse_err(b"@b = global bfloat 1.0039062500000000000000000000000001\n"),
        "floating point constant invalid for type"
    );
}

/// `fp128`, `x86_fp80` and `ppc_fp128` have **no decimal spelling** upstream,
/// and this is the message that says so — the one `convertValIDToValue`
/// reaches after `isValueValidForType` has passed. The lexer hands it a
/// `double`; the narrowing step deliberately covers only half / bfloat /
/// single ("Long double does not need this"), so the value is still a double
/// when its type is compared against the demanded one.
///
/// The hex forms are how these types are written, and
/// `exotic_hex_float_literals_round_trip_bits` covers them. No upstream `.ll`
/// pins this message — none of them writes a decimal at these types, which is
/// itself the evidence — so the guard is anchored by symbol.
#[test]
fn the_wide_float_types_have_no_decimal_spelling() {
    for (source, expected) in [
        (
            b"@q = global fp128 1.0000000000000001\n".as_slice(),
            "floating point constant does not have type 'fp128'",
        ),
        (
            b"@x = global x86_fp80 1.0000000000000001\n".as_slice(),
            "floating point constant does not have type 'x86_fp80'",
        ),
        (
            b"@p = global ppc_fp128 1.0000000000000001\n".as_slice(),
            "floating point constant does not have type 'ppc_fp128'",
        ),
    ] {
        assert_eq!(parse_err(source), expected);
    }
}

/// Port of `LLLexer.cpp` hex APFloat token forms and parser semantic lowering.
///
/// Every form now round-trips to **its own spelling**, which is the property
/// `LLLexer::HexToIntPair` and `AsmWriter` give upstream. `0xL` and `0xM`
/// previously came back with their two 64-bit halves transposed, because the
/// parser read the digits as one big-endian 128-bit number where upstream
/// reads the first sixteen into the *low* word.
#[test]
fn exotic_hex_float_literals_round_trip_bits() {
    let text = parse_and_render(
        "exotic_hex_float_literals_round_trip_bits",
        b"@h = global half 0xH3c00\n@b = global bfloat 0xR3f80\n@q = global fp128 0xL3fff0000000000000000000000000000\n@x = global x86_fp80 0xK3fff8000000000000000\n@p = global ppc_fp128 0xM3ff00000000000000000000000000000\n",
    );
    assert!(text.contains("@h = global half 0xH3C00"), "{text}");
    assert!(text.contains("@b = global bfloat 0xR3F80"), "{text}");
    assert!(
        text.contains("@q = global fp128 0xL3FFF0000000000000000000000000000"),
        "{text}"
    );
    assert!(
        text.contains("@x = global x86_fp80 0xK3FFF8000000000000000"),
        "{text}"
    );
    assert!(
        text.contains("@p = global ppc_fp128 0xM3FF00000000000000000000000000000"),
        "{text}"
    );
}

/// Port of `LLParser.cpp::parseValID` APFloat typed lowering: untyped hex
/// floating literals are double-semantics tokens converted to the requested
/// float type by context.
#[test]
fn hex_double_literal_converts_to_float_context() {
    let text = parse_and_render(
        "hex_double_literal_converts_to_float_context",
        b"@g = global float 0x400921fb60000000\n",
    );
    // `writeAPFloatInternal` — reached from `writeConstantInternal`'s
    // `ConstantFP` arm, which holds the vector `splat (` wrapper around the
    // delegating call — prints the hex form through
    // `format_hex(bits, 0, /*Upper=*/true)`, so the digits come back
    // uppercase whatever case the source used.
    assert!(
        text.contains("@g = global float 0x400921FB60000000"),
        "{text}"
    );
}

/// Mirrors `test/Assembler/2002-04-07-InfConstant.ll`'s
/// `; CHECK: fmul float 0x7FF0000000000000, 1.000000e+01` (RUN:
/// `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s`, so the
/// CHECK line is `AssemblyWriter` output). The statement is
/// `Out << format_hex(apf.bitcastToAPInt().getZExtValue(), 0, /*Upper=*/true)`
/// in `llvm/lib/IR/AsmWriter.cpp::writeAPFloatInternal`, a file-static free
/// function reached from `writeConstantInternal`'s `ConstantFP` arm — that arm
/// wraps the delegating call in the vector `splat (…)` form and does not print
/// the digits itself, so it is `writeAPFloatInternal` a porter should grep for.
#[test]
fn hex_float_constants_print_uppercase() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/2002-04-07-InfConstant.ll");

    let text = parse_and_render("hex_float_constants_print_uppercase", FIXTURE);
    assert!(
        text.contains("fmul float 0x7FF0000000000000, 1.000000e+01"),
        "{text}"
    );
}

/// Mirrors `test/Assembler/2002-04-07-HexFloatConstants.ll`'s
/// `fmul double 7.200000e+101, 0x427F4000`.
///
/// **Caveat on the oracle, stated rather than implied.** That fixture's RUN
/// lines are `opt -passes=instsimplify -S` and
/// `llvm-as | llvm-dis | llvm-as | opt | llvm-dis` followed by `diff` — there
/// is no `FileCheck`, so what the fixture actually pins is printer
/// *idempotence*, and `0x427F4000` is its hand-written **input** text, not a
/// captured `llvm-dis` output. The rule this test asserts comes from the
/// routine: `writeAPFloatInternal` (reached from `writeConstantInternal`'s
/// `ConstantFP` arm) prints the hex form as
/// `format_hex(bits, /*Width=*/0, /*Upper=*/true)`, which reaches
/// `llvm::write_hex` with `NumChars = max(W, max(1, Nibbles) + PrefixChars)`,
/// so with `W == 0` a value of eight significant nibbles prints in eight
/// digits and is *not* padded to sixteen. The fixture is the shape and the
/// spelling; `write_hex` is the authority. (Its sibling
/// `hex_float_constants_print_uppercase` does have a real FileCheck oracle.)
#[test]
fn hex_float_constants_are_not_zero_padded_past_their_width() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/2002-04-07-HexFloatConstants.ll");

    let text = parse_and_render(
        "hex_float_constants_are_not_zero_padded_past_their_width",
        FIXTURE,
    );
    assert!(
        text.contains("fmul double 7.200000e+101, 0x427F4000"),
        "{text}"
    );
}

/// `LLParser::parseUInt64` gates on the token kind and the APSInt's
/// **signedness**, never on its spelling:
///
/// ```text
/// if (Lex.getKind() != lltok::APSInt || Lex.getAPSIntVal().isSigned())
///   return tokError("expected integer");
/// Val = Lex.getAPSIntVal().getLimitedValue();
/// ```
///
/// `LLLexer::lexIdentifier`'s `[us]0x[0-9A-Fa-f]+` block stamps
/// `APSInt(Tmp, TokStart[0] == 'u')`, so `u0x…` is unsigned and passes both
/// gates wherever a `uint64` is wanted. `align` and `dereferenceable(N)` both
/// read through `parseUInt64`.
///
/// Anchored on the routine, not a fixture: `grep -rlaE '[us]0x' test/Assembler
/// test/Verifier` under `orig_cpp/llvm-project-llvmorg-22.1.4/llvm` matches
/// `invalid-hexint.ll` and `matrix-intrinsics.ll` only, and both write the
/// literal in a *value* position, which `parseValID` reads — not in one of
/// `parseUInt64`'s.
#[test]
fn an_unsigned_hex_literal_is_accepted_wherever_a_uint64_is_wanted() {
    let text = parse_and_render(
        "uint64_u0x",
        b"@g = global i32 0, align u0x4\n\
define void @f(ptr align u0x8 dereferenceable(u0x10) %p) {\n\
  ret void\n\
}\n"
        .as_slice(),
    );
    assert!(text.contains("@g = global i32 0, align 4"), "got:\n{text}");
    assert!(
        text.contains("ptr align 8 dereferenceable(16) %p"),
        "got:\n{text}"
    );
}

/// `parseUInt64` reads the value with `APSInt::getLimitedValue()`, whose
/// default limit is `UINT64_MAX` and which **saturates** (`APInt::ugt(Limit) ?
/// Limit : getZExtValue()`) rather than failing. So a literal too wide for 64
/// bits is not a lexical error at all: it reaches
/// `parseOptionalAlignment`'s own checks as `UINT64_MAX`, which is not a power
/// of two.
///
/// The message therefore has to be `alignment is not a power of two`, not
/// `expected integer` — the divergence was observable in the *diagnostic*, not
/// only in an internal value.
///
/// Anchored on the routine; the search above finds no fixture that writes an
/// over-wide literal in a `parseUInt64` position.
#[test]
fn an_over_wide_alignment_saturates_instead_of_failing_to_parse() {
    let err = parse_err(
        b"define void @f() {\n\
  %a = alloca i32, align 99999999999999999999999\n\
  ret void\n\
}\n"
        .as_slice(),
    );
    assert_eq!(err, "alignment is not a power of two");
}

/// `parseUInt32` shares `parseUInt64`'s guard and adds a range check *after*
/// the saturating read:
///
/// ```text
/// uint64_t Val64 = Lex.getAPSIntVal().getLimitedValue(0xFFFFFFFFULL+1);
/// if (Val64 != unsigned(Val64)) return tokError("expected 32-bit integer (too large)");
/// ```
///
/// Two consequences are pinned here. An `addrspace(u0x1)` is accepted, since
/// the guard never looks at the spelling; and `align = 4294967296` inside an
/// attribute group still answers the second message, because saturation lands
/// on `0x100000000`, which is exactly what the range check rejects. Neither
/// message may collapse into the other.
#[test]
fn parse_uint32_takes_an_unsigned_hex_literal_and_keeps_its_range_message() {
    let text = parse_and_render(
        "uint32_u0x",
        b"@g = addrspace(u0x1) global i32 0\n@h = global ptr addrspace(u0x2) null\n".as_slice(),
    );
    assert!(
        text.contains("@g = addrspace(1) global i32 0"),
        "got:\n{text}"
    );
    assert!(
        text.contains("@h = global ptr addrspace(2) null"),
        "got:\n{text}"
    );

    let err = parse_err(
        b"define void @f() {\n  ret void\n}\nattributes #0 = { align = 4294967296 }\n".as_slice(),
    );
    assert_eq!(err, "expected 32-bit integer (too large)");
}

/// The guard is on `APSInt::isSigned()`, so the *signed* spellings stay
/// rejected: a negative decimal and `s0x…` both answer `expected integer`,
/// which is the message `test/Assembler/align-param-attr-error2.ll` pins for
/// the neighbouring empty-parens case.
#[test]
fn a_signed_literal_is_still_not_a_uint64() {
    assert_eq!(
        parse_err(b"@g = global i32 0, align -4\n".as_slice()),
        "expected integer"
    );
    assert_eq!(
        parse_err(b"@g = global i32 0, align s0x4\n".as_slice()),
        "expected integer"
    );
}
