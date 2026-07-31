//! Word order of the 128-bit hexadecimal float literals (`0xL`, `0xM`).
//!
//! LLVM does **not** spell these as one big-endian 128-bit number. `LLLexer`'s
//! `HexToIntPair` (`LLLexer.cpp:86`) reads the first sixteen hex digits into
//! `Pair[0]` — the APInt's *low* word — and the second sixteen into `Pair[1]`,
//! the high word. `AsmWriter` prints them back in that same order,
//! `getLoBits(64)` then `getHiBits(64)` (`AsmWriter.cpp:1605-1616`). So the
//! low half is written first.
//!
//! `0xK` (`x86_fp80`) is the exception in the other direction: `FP80HexToIntPair`
//! (`LLLexer.cpp:107`) reads the first *four* digits into the high word and the
//! next sixteen into the low word, and `AsmWriter` prints `getHiBits(16)` then
//! `getLoBits(64)` — which is exactly a plain big-endian 80-bit number.
//!
//! Both forms used to come back with their halves transposed, for different
//! reasons, so both are pinned here:
//!
//! - **`fp128`** was read as one big-endian 128-bit number, which is simply not
//!   the format. That was fixed in the *parser* (`parse_hex_apfloat_pair`), and
//!   it changed values as well as spelling: `0xL0…03FFF0…0` is 1.0 upstream and
//!   was being read as a subnormal.
//! - **`ppc_fp128`** parsed correctly by accident — llvmkit stores the
//!   component pair mirrored from upstream (see
//!   `llvmkit-ir/tests/ap_float_ppc_word_order.rs`), and the two mirrorings
//!   cancelled. Its *printer* was the half that disagreed, so that is where it
//!   was fixed. Values were never wrong here; only the printed spelling was.

use llvmkit_asmparser::parse_dynamic;

/// `parse → print` must reach a fixed point immediately. A parser and printer
/// that disagree about word order oscillate between two spellings, which this
/// catches without having to decide which spelling is the right one.
fn assert_print_is_idempotent(label: &str, src: &str) {
    let first = format!(
        "{}",
        parse_dynamic(src).unwrap_or_else(|e| panic!("{label}: parse failed: {e}\n{src}"))
    );
    let second = format!(
        "{}",
        parse_dynamic(first.as_str())
            .unwrap_or_else(|e| panic!("{label}: re-parse failed: {e}\n{first}"))
    );
    assert_eq!(
        first, second,
        "{label}: printing is not idempotent — the parser and the printer \
         disagree about word order"
    );
}

#[test]
fn hex_float_printing_is_idempotent() {
    for (label, src) in [
        ("half", "@h = global half 0xH3C00\n"),
        ("bfloat", "@b = global bfloat 0xR3F80\n"),
        ("double", "@d = global double 0x3FF0000000000000\n"),
        ("x86_fp80", "@x = global x86_fp80 0xK3FFF8000000000000000\n"),
        (
            "fp128",
            "@q = global fp128 0xL00000000000000003FFF000000000000\n",
        ),
        (
            "ppc_fp128",
            "@p = global ppc_fp128 0xM3FF00000000000000000000000000000\n",
        ),
    ] {
        assert_print_is_idempotent(label, src);
    }
}

/// The spelling LLVM's own test suite uses for `fp128` 1.0 is
/// `0xL00000000000000003FFF000000000000`: low word first, so the exponent
/// field lands in the *high* word. Read as a plain big-endian number this is a
/// subnormal instead, which is how the transposition shows up as a wrong value
/// rather than only as wrong text.
#[test]
fn fp128_one_uses_upstream_word_order() {
    let src = "@q = global fp128 0xL00000000000000003FFF000000000000\n";
    let printed = format!("{}", parse_dynamic(src).expect("fp128 1.0 parses"));
    assert!(
        printed.contains("0xL00000000000000003FFF000000000000"),
        "fp128 1.0 did not survive the round trip:\n{printed}"
    );
}

/// Same check on the `ppc_fp128` side. Upstream writes the **leading** double
/// first — its low word holds `DoubleAPFloat::Floats[0]` and `AsmWriter` prints
/// `getLoBits(64)` first — so `0xM3FF0000000000000...` is 1.0.
#[test]
fn ppc_fp128_one_uses_upstream_word_order() {
    let src = "@p = global ppc_fp128 0xM3FF00000000000000000000000000000\n";
    let printed = format!("{}", parse_dynamic(src).expect("ppc_fp128 1.0 parses"));
    assert!(
        printed.contains("0xM3FF00000000000000000000000000000"),
        "ppc_fp128 1.0 did not survive the round trip:\n{printed}"
    );
}

/// `x86_fp80` is a plain big-endian 80-bit number in both directions, so it
/// must keep working unchanged — the fix for the 128-bit forms must not be
/// applied to it.
#[test]
fn x86_fp80_stays_big_endian() {
    let src = "@x = global x86_fp80 0xK3FFF8000000000000000\n";
    let printed = format!("{}", parse_dynamic(src).expect("x86_fp80 1.0 parses"));
    assert!(
        printed.contains("0xK3FFF8000000000000000"),
        "x86_fp80 1.0 did not survive the round trip:\n{printed}"
    );
}
