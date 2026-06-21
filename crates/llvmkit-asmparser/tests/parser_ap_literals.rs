//! Parser coverage for APInt/APFloat-backed numeric literals.
//!
//! Ports `LLLexer.cpp` numeric token forms and `LLParser.cpp::parseValID`
//! conversion through typed constants.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_render(module_name: &str, src: &[u8]) -> String {
    Module::with_new(module_name, |module| {
        Parser::new(src, &module)
            .expect("lexer primes")
            .parse_module()
            .expect("parser succeeds");
        format!("{module}")
    })
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

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal half tokens round in half destination semantics, not host `double`.
#[test]
fn decimal_half_literal_rounds_in_half_semantics() {
    let text = parse_and_render(
        "decimal_half_literal_rounds_in_half_semantics",
        b"@h = global half 1.0004882812500000000000000000000001\n",
    );
    assert!(text.contains("@h = global half 0xH3C01"), "{text}");
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal bfloat tokens round in bfloat destination semantics, not host `double`.
#[test]
fn decimal_bfloat_literal_rounds_in_bfloat_semantics() {
    let text = parse_and_render(
        "decimal_bfloat_literal_rounds_in_bfloat_semantics",
        b"@b = global bfloat 1.0039062500000000000000000000000001\n",
    );
    assert!(text.contains("@b = global bfloat 0xR3F81"), "{text}");
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal fp128 tokens preserve significand bits below host `double` precision.
#[test]
fn decimal_fp128_literal_keeps_bits_beyond_host_double() {
    let text = parse_and_render(
        "decimal_fp128_literal_keeps_bits_beyond_host_double",
        b"@q = global fp128 1.0000000000000001\n",
    );
    assert!(
        text.contains("@q = global fp128 0xL0734ACA5F6226F0B3FFF000000000000"),
        "{text}"
    );
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::IEEEFloat::convertFromDecimalString`:
/// decimal x86_fp80 tokens preserve significand bits below host `double` precision.
#[test]
fn decimal_x86_fp80_literal_keeps_bits_beyond_host_double() {
    let text = parse_and_render(
        "decimal_x86_fp80_literal_keeps_bits_beyond_host_double",
        b"@x = global x86_fp80 1.0000000000000001\n",
    );
    assert!(
        text.contains("@x = global x86_fp80 0xK3FFF800000000000039A"),
        "{text}"
    );
}

/// llvmkit-specific subset of
/// `llvm/lib/Support/APFloat.cpp::DoubleAPFloat::convertFromString`:
/// decimal ppc_fp128 tokens keep the low double component below host precision.
#[test]
fn decimal_ppc_fp128_literal_keeps_low_component_beyond_host_double() {
    let text = parse_and_render(
        "decimal_ppc_fp128_literal_keeps_low_component_beyond_host_double",
        b"@p = global ppc_fp128 1.0000000000000001\n",
    );
    assert!(
        text.contains("@p = global ppc_fp128 0xM3C9CD2B297D889BC3FF0000000000000"),
        "{text}"
    );
}

/// Port of `LLLexer.cpp` hex APFloat token forms and parser semantic lowering.
#[test]
fn exotic_hex_float_literals_round_trip_bits() {
    let text = parse_and_render(
        "exotic_hex_float_literals_round_trip_bits",
        b"@h = global half 0xH3c00\n@b = global bfloat 0xR3f80\n@q = global fp128 0xL3fff0000000000000000000000000000\n@x = global x86_fp80 0xK3fff8000000000000000\n@p = global ppc_fp128 0xM3ff00000000000000000000000000000\n",
    );
    assert!(text.contains("@h = global half 0xH3C00"), "{text}");
    assert!(text.contains("@b = global bfloat 0xR3F80"), "{text}");
    assert!(
        text.contains("@q = global fp128 0xL00000000000000003FFF000000000000"),
        "{text}"
    );
    assert!(
        text.contains("@x = global x86_fp80 0xK3FFF8000000000000000"),
        "{text}"
    );
    assert!(
        text.contains("@p = global ppc_fp128 0xM00000000000000003FF0000000000000"),
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
