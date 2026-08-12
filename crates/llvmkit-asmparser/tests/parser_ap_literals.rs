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
/// tree's only `[us]0x` fixture and it turns on a malformed token, which is
/// blocked on the lexer's error-token layering. Anchored by symbol instead.
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
    assert!(
        text.contains("@g = global float 0x400921fb60000000"),
        "{text}"
    );
}
