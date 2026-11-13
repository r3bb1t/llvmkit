//! [`DataLayout`] parser + accessor tests, ported from
//! `unittests/IR/DataLayoutTest.cpp`.
//!
//! ## Upstream provenance
//!
//! Each test cites a specific `TEST(DataLayout*, ...)` block. The Task 7
//! audited diagnostics below compare llvmkit's exact [`DataLayoutError`]
//! reason string; rows are marked `llvmkit-specific subset` when that string
//! intentionally differs from upstream `FailedWithMessage` text.

use llvmkit_ir::{
    Align, ArrayType, DataLayout, DataLayoutError, IrError, ManglingMode, MaybeAlign, Module,
    SizedType, StructType, Type, TypeKindLabel, TypeSize, VectorType, address_space, lanes,
    module_new,
};

fn parse(s: &str) -> DataLayout {
    DataLayout::parse(s).unwrap_or_else(|e| panic!("parse {s:?}: {e:?}"))
}

/// Port of `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, GlobalsAddressSpace)`.
/// `new GlobalVariable(*M, Int32, false, ExternalLinkage, nullptr)` is
/// `global_builder("", int32).build()`, whose defaults are those arguments.
#[test]
fn upstream_globals_address_space() {
    // When not explicitly defined the globals address space should be zero:
    assert_eq!(parse("").default_globals_addr_space(), 0);
    assert_eq!(parse("P1-A2").default_globals_addr_space(), 0);
    assert_eq!(parse("G2").default_globals_addr_space(), 2);
    // Check that creating a GlobalVariable without an explicit address space
    // in a module with a default globals address space respects that default:
    let m = Module::dynamic("MyModule");
    // Default is globals in address space zero:
    let int32 = m.i32_type();
    let default_global1 = m.global_builder("", int32).build().expect("unnamed");
    assert_eq!(m.view(default_global1).address_space(), 0);
    let explicit_global1 = m
        .global_builder("", int32)
        .address_space(address_space::<123>())
        .build()
        .expect("unnamed");
    assert_eq!(m.view(explicit_global1).address_space(), 123);

    // When using a datalayout with the global address space set to 200, global
    // variables should default to 200
    m.set_data_layout(parse("G200"));
    let default_global2 = m.global_builder("", int32).build().expect("unnamed");
    assert_eq!(m.view(default_global2).address_space(), 200);
    let explicit_global2 = m
        .global_builder("", int32)
        .address_space(address_space::<123>())
        .build()
        .expect("unnamed");
    assert_eq!(m.view(explicit_global2).address_space(), 123);
}

fn parse_err(s: &str) -> String {
    match DataLayout::parse(s) {
        Ok(_) => panic!("expected error for {s:?}"),
        // Exhaustive, no `_` arm: pins `DataLayoutError` to its one shape —
        // a second variant would stop this from compiling.
        Err(DataLayoutError { reason }) => reason,
    }
}

fn assert_parse_err_eq(s: &str, expected: &str) {
    assert_eq!(parse_err(s), expected, "{s}");
}

fn assert_line(text: &str, expected: &str) {
    for line in text.lines() {
        if line == expected {
            return;
        }
    }
    panic!("missing line `{expected}` in:\n{text}");
}

// ---------------------------------------------------------------------------
// Layout string format / framing
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, LayoutStringFormat)`.
#[test]
fn layout_string_format_accepts_well_formed() {
    for s in ["", "e", "m:e", "m:e-e"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` rejection arm from `LayoutStringFormat`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn layout_string_format_rejects_empty_specs() {
    for s in ["-", "e-", "-m:e", "m:e--e"] {
        assert_parse_err_eq(s, "empty specification is not allowed");
    }
}

/// `llvmkit-specific subset` of
/// `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, InvalidSpecifier)`;
/// asserts llvmkit's exact diagnostics for the upstream invalid strings.
#[test]
fn invalid_specifier_rejected() {
    for (input, expected) in [
        ("^", "unknown specifier '^'"),
        ("I8:8", "unknown specifier 'I'"),
        ("e-X", "unknown specifier 'X'"),
        ("p0:32:32-64", "unknown specifier '6'"),
    ] {
        assert_parse_err_eq(input, expected);
    }
}

// ---------------------------------------------------------------------------
// Endianness
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseEndianness)`.
#[test]
fn parse_endianness_round_trip() {
    for s in ["e", "E"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` rejection arm from `ParseEndianness`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_endianness_rejects_extra_chars() {
    for s in ["ee", "e0", "e:0", "E0:E", "El", "E:B"] {
        assert_parse_err_eq(s, "malformed specification, must be just 'e' or 'E'");
    }
}

// ---------------------------------------------------------------------------
// Mangling
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseMangling)`.
#[test]
fn parse_mangling_modes() {
    for s in ["m:a", "m:e", "m:l", "m:m", "m:o", "m:w", "m:x"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` malformed arm from `ParseMangling`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_mangling_rejects_malformed() {
    for s in ["m", "ms:m", "m:"] {
        assert_parse_err_eq(s, "malformed specification, expected m:<mangling>");
    }
}

/// `llvmkit-specific subset` unknown-mode arm from `ParseMangling`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_mangling_rejects_unknown_mode() {
    for s in ["m:ms", "m:E", "m:0"] {
        assert_parse_err_eq(s, "unknown mangling mode");
    }
}

// ---------------------------------------------------------------------------
// Stack natural alignment
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseStackNaturalAlign)`.
#[test]
fn parse_stack_natural_align_accepts() {
    for s in ["S8", "S32768"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` empty arm from `ParseStackNaturalAlign`;
/// asserts llvmkit's exact diagnostic.
#[test]
fn parse_stack_natural_align_rejects_empty() {
    assert_parse_err_eq("S", "malformed specification, expected S<size>");
}

/// `llvmkit-specific subset` bad-integer arm from `ParseStackNaturalAlign`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_stack_natural_align_rejects_bad_int() {
    for s in ["SX", "S0x20", "S65536"] {
        assert_parse_err_eq(s, "stack natural alignment must be a 16-bit integer");
    }
}

/// `llvmkit-specific subset` zero arm from `ParseStackNaturalAlign`;
/// asserts llvmkit's exact diagnostic.
#[test]
fn parse_stack_natural_align_rejects_zero() {
    assert_parse_err_eq("S0", "stack natural alignment must be non-zero");
}

/// `llvmkit-specific subset` non-power-of-two arm from
/// `ParseStackNaturalAlign`; asserts llvmkit's exact diagnostic for upstream
/// malformed strings.
#[test]
fn parse_stack_natural_align_rejects_non_power_of_two_byte_multiple() {
    for s in ["S1", "S7", "S24", "S65535"] {
        assert_parse_err_eq(
            s,
            "stack natural alignment must be a power of two times the byte width",
        );
    }
}

// ---------------------------------------------------------------------------
// AddrSpace specifiers (P / A / G)
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseAddrSpace)`.
#[test]
fn parse_addr_space_specifiers() {
    for s in [
        "P0",
        "A0",
        "G0",
        "P1",
        "A1",
        "G1",
        "P16777215",
        "A16777215",
        "G16777215",
    ] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` missing-value arm from `ParseAddrSpace`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_addr_space_rejects_missing_value() {
    for (s, expected) in [
        ("P", "malformed specification, expected P<address space>"),
        ("A", "malformed specification, expected A<address space>"),
        ("G", "malformed specification, expected G<address space>"),
    ] {
        assert_parse_err_eq(s, expected);
    }
}

/// `llvmkit-specific subset` bad-value arm from `ParseAddrSpace`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_addr_space_rejects_bad_value() {
    for s in ["Px", "A0x1", "G16777216"] {
        assert_parse_err_eq(s, "address space must be a 24-bit integer");
    }
}

/// `parseAddrSpace`, `parseSize`, `parseAlignment` and the aggregate spec each
/// read a number with `to_integer(Str, N, 10)`, and its
/// `consumeUnsignedInteger` (`lib/Support/StringRef.cpp`) has no sign arm, so a
/// leading `+` is a byte it cannot consume. llvmkit's `str::parse` takes one.
/// llvmkit-specific (rule anchor): `TEST(DataLayout, ...)` has no signed number.
#[test]
fn a_layout_number_takes_no_sign() {
    let cases = [
        ("P+5", "address space must be a 24-bit integer"),
        ("A+5", "address space must be a 24-bit integer"),
        ("G+5", "address space must be a 24-bit integer"),
        ("p+1:64:64", "address space must be a 24-bit integer"),
        ("ni:+1", "address space must be a 24-bit integer"),
        ("i+8:8", "size must be a non-zero 24-bit integer"),
        ("p:+64:64", "pointer size must be a non-zero 24-bit integer"),
        ("a+0:8", "size must be zero"),
        ("i8:+8", "ABI alignment must be a 16-bit integer"),
        ("S+8", "stack natural alignment must be a 16-bit integer"),
    ];
    let accepted: Vec<&str> = cases
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| DataLayout::parse(s).is_ok())
        .collect();
    assert!(accepted.is_empty(), "accepted {accepted:?}");
    for (s, expected) in cases {
        assert_parse_err_eq(s, expected);
    }
}

// ---------------------------------------------------------------------------
// Function-pointer spec
// ---------------------------------------------------------------------------

/// `llvmkit-specific subset` of
/// `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseFuncPtrSpec)`:
/// retains every upstream accepted/rejected string, but compares llvmkit's
/// exact diagnostic strings where wording differs from upstream.
#[test]
fn parse_func_ptr_spec() {
    for s in ["Fi8", "Fn16", "Fi32768", "Fn32768"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }

    assert_parse_err_eq("F", "malformed specification, expected F<type><abi>");
    assert_parse_err_eq("FN", "unknown function pointer alignment type 'N'");
    assert_parse_err_eq("F32", "unknown function pointer alignment type '3'");

    for s in ["Fi", "Fn"] {
        assert_parse_err_eq(s, "ABI alignment component cannot be empty");
    }
    for s in ["Fii", "Fn32x", "Fi65536", "Fn65536"] {
        assert_parse_err_eq(s, "ABI alignment must be a 16-bit integer");
    }
    for s in ["Fi0", "Fn0"] {
        assert_parse_err_eq(s, "ABI alignment must be non-zero");
    }
    for s in ["Fi12", "Fn24"] {
        assert_parse_err_eq(
            s,
            "ABI alignment must be a power of two times the byte width",
        );
    }
}

// ---------------------------------------------------------------------------
// Native integer widths
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayoutTest, ParseNativeIntegersSpec)`.
#[test]
fn parse_native_integers_spec_accepts() {
    for s in ["n1", "n1:8", "n24:12:16777215"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` empty-component arm from
/// `ParseNativeIntegersSpec`; asserts llvmkit's exact diagnostic for upstream
/// malformed strings.
#[test]
fn parse_native_integers_spec_rejects_empty_components() {
    for s in ["n", "n1:", "n:8", "n16::32"] {
        assert_parse_err_eq(s, "size component cannot be empty");
    }
}

/// `llvmkit-specific subset` zero/bad-integer arm from
/// `ParseNativeIntegersSpec`; asserts llvmkit's exact diagnostic for upstream
/// malformed strings.
#[test]
fn parse_native_integers_spec_rejects_zero_or_huge() {
    for s in [
        "n0",
        "n0x8:16",
        "n8:0",
        "n16:0:32",
        "n16777216",
        "n16:16777216",
        "n32:64:16777216",
    ] {
        assert_parse_err_eq(s, "size must be a non-zero 24-bit integer");
    }
}

// ---------------------------------------------------------------------------
// Non-integral address spaces (ni)
// ---------------------------------------------------------------------------

/// `llvmkit-specific subset` of
/// `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, ParseNonIntegralAddrSpace)`:
/// retains every upstream accepted/rejected string, but compares llvmkit's
/// exact diagnostic strings where wording differs from upstream.
#[test]
fn parse_non_integral_addr_space_accepts() {
    for s in ["ni:1", "ni:16777215", "ni:1:16777215"] {
        DataLayout::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
}

/// `llvmkit-specific subset` malformed arm from `ParseNonIntegralAddrSpace`;
/// asserts llvmkit's exact diagnostic for the upstream malformed strings.
#[test]
fn parse_non_integral_addr_space_rejects_malformed() {
    for s in ["ni", "ni42", "nix"] {
        assert_parse_err_eq(
            s,
            "malformed specification, expected ni:<address space>[:<address space>]...",
        );
    }
}

/// `llvmkit-specific subset` zero-address-space arm from
/// `ParseNonIntegralAddrSpace`; asserts llvmkit's exact diagnostic.
#[test]
fn parse_non_integral_addr_space_rejects_zero() {
    for s in ["ni:0", "ni:42:0"] {
        assert_parse_err_eq(s, "address space 0 cannot be non-integral");
    }
}

/// `llvmkit-specific subset` empty-component arm from
/// `ParseNonIntegralAddrSpace`; asserts llvmkit's exact diagnostic.
#[test]
fn parse_non_integral_addr_space_rejects_empty_components() {
    for s in ["ni:", "ni::42", "ni:42:"] {
        assert_parse_err_eq(s, "address space component cannot be empty");
    }
}

/// `llvmkit-specific subset` bad-integer arm from
/// `ParseNonIntegralAddrSpace`; asserts llvmkit's exact diagnostic.
#[test]
fn parse_non_integral_addr_space_rejects_bad_int() {
    for s in ["ni:x", "ni:42:0x1", "ni:16777216", "ni:42:16777216"] {
        assert_parse_err_eq(s, "address space must be a 24-bit integer");
    }
}

// ---------------------------------------------------------------------------
// Stack alignment table
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetStackAlignment)`.
#[test]
fn get_stack_alignment_default_unset() {
    let dl = DataLayout::default();
    assert!(dl.stack_alignment().is_none());
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetStackAlignment)`.
#[test]
fn get_stack_alignment_table() {
    let cases: &[(&str, u64)] = &[("S8", 1), ("S64", 8), ("S32768", 4096)];
    for (s, want) in cases {
        let dl = parse(s);
        assert_eq!(
            dl.stack_alignment(),
            Some(Align::new(*want).expect("a")),
            "{s}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pointer size / index size / alignment tables
// ---------------------------------------------------------------------------

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetPointerSizeInBits)`.
#[test]
fn get_pointer_size_in_bits_table() {
    let cases: &[(&str, u32, u32, u32)] = &[
        ("", 64, 64, 64),
        ("p:16:32", 16, 16, 16),
        ("p0:32:64", 32, 32, 32),
        ("p1:16:32", 64, 16, 64),
        ("p1:31:32-p2:15:16:16:14", 64, 31, 15),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(
            dl.pointer_size_in_bits(address_space::<0>()),
            *v0,
            "{s} AS 0"
        );
        assert_eq!(
            dl.pointer_size_in_bits(address_space::<1>()),
            *v1,
            "{s} AS 1"
        );
        assert_eq!(
            dl.pointer_size_in_bits(address_space::<2>()),
            *v2,
            "{s} AS 2"
        );
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetPointerSize)`.
#[test]
fn get_pointer_size_table() {
    let cases: &[(&str, u32, u32, u32)] = &[
        ("", 8, 8, 8),
        ("p:16:32", 2, 2, 2),
        ("p0:32:64", 4, 4, 4),
        ("p1:17:32", 8, 3, 8),
        ("p1:31:64-p2:23:8:16:9", 8, 4, 3),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(dl.pointer_size(address_space::<0>()), *v0, "{s} AS 0");
        assert_eq!(dl.pointer_size(address_space::<1>()), *v1, "{s} AS 1");
        assert_eq!(dl.pointer_size(address_space::<2>()), *v2, "{s} AS 2");
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetIndexSizeInBits)`.
#[test]
fn get_index_size_in_bits_table() {
    let cases: &[(&str, u32, u32, u32)] = &[
        ("", 64, 64, 64),
        ("p:16:32", 16, 16, 16),
        ("p0:32:64", 32, 32, 32),
        ("p1:16:32:32:10", 64, 10, 64),
        ("p1:31:32:64:20-p2:17:16:16:15", 64, 20, 15),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(dl.index_size_in_bits(address_space::<0>()), *v0, "{s} AS 0");
        assert_eq!(dl.index_size_in_bits(address_space::<1>()), *v1, "{s} AS 1");
        assert_eq!(dl.index_size_in_bits(address_space::<2>()), *v2, "{s} AS 2");
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetIndexSize)`.
#[test]
fn get_index_size_table() {
    let cases: &[(&str, u32, u32, u32)] = &[
        ("", 8, 8, 8),
        ("p:16:32", 2, 2, 2),
        ("p0:27:64", 4, 4, 4),
        ("p1:19:32:64:5", 8, 1, 8),
        ("p1:33:32:64:23-p2:21:8:16:13", 8, 3, 2),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(dl.index_size(address_space::<0>()), *v0, "{s} AS 0");
        assert_eq!(dl.index_size(address_space::<1>()), *v1, "{s} AS 1");
        assert_eq!(dl.index_size(address_space::<2>()), *v2, "{s} AS 2");
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetPointerABIAlignment)`.
#[test]
fn get_pointer_abi_alignment_table() {
    let cases: &[(&str, u64, u64, u64)] = &[
        ("", 8, 8, 8),
        ("p:16:32", 4, 4, 4),
        ("p0:16:32:64", 4, 4, 4),
        ("p1:32:16:64", 8, 2, 8),
        ("p1:33:16:32:15-p2:23:8:16:9", 8, 2, 1),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(
            dl.pointer_abi_align(address_space::<0>()).value(),
            *v0,
            "{s} AS 0"
        );
        assert_eq!(
            dl.pointer_abi_align(address_space::<1>()).value(),
            *v1,
            "{s} AS 1"
        );
        assert_eq!(
            dl.pointer_abi_align(address_space::<2>()).value(),
            *v2,
            "{s} AS 2"
        );
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, GetPointerPrefAlignment)`.
#[test]
fn get_pointer_pref_alignment_table() {
    let cases: &[(&str, u64, u64, u64)] = &[
        ("", 8, 8, 8),
        ("p:16:32", 4, 4, 4),
        ("p0:8:16:32", 4, 4, 4),
        ("p1:32:8:16", 8, 2, 8),
        ("p1:33:8:16:31-p2:23:8:32:17", 8, 2, 4),
    ];
    for (s, v0, v1, v2) in cases {
        let dl = parse(s);
        assert_eq!(
            dl.pointer_pref_align(address_space::<0>()).value(),
            *v0,
            "{s} AS 0"
        );
        assert_eq!(
            dl.pointer_pref_align(address_space::<1>()).value(),
            *v1,
            "{s} AS 1"
        );
        assert_eq!(
            dl.pointer_pref_align(address_space::<2>()).value(),
            *v2,
            "{s} AS 2"
        );
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, AddressSpaceName)`.
#[test]
fn address_space_name() {
    let dl = parse("p:16:32-p1(foo):16:32-p10(bar):16:16");
    assert_eq!(dl.address_space_name(address_space::<0>()), "");
    assert_eq!(dl.address_space_name(address_space::<1>()), "foo");
    assert_eq!(dl.address_space_name(address_space::<10>()), "bar");
    assert_eq!(dl.address_space_name(address_space::<3>()), "");
    assert_eq!(
        dl.named_address_space("foo"),
        llvmkit_ir::AddressSpace::new(1)
    );
    assert_eq!(
        dl.named_address_space("bar"),
        llvmkit_ir::AddressSpace::new(10)
    );
    assert_eq!(dl.named_address_space("missing"), None);
}

/// Port of `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, IsNonIntegralAddressSpace)`,
/// whole and in order.
#[test]
fn is_non_integral_address_space() {
    let two = address_space::<2>();
    let top = address_space::<16777215>();
    let spaces = |dl: &DataLayout| dl.non_standard_address_spaces().collect::<Vec<_>>();

    let default = DataLayout::default();
    assert!(spaces(&default).is_empty());
    assert!(!default.is_non_integral_address_space(address_space::<0>()));
    assert!(!default.is_non_integral_address_space(address_space::<1>()));

    let custom = parse("ni:2:16777215");
    assert_eq!(spaces(&custom), [two, top]);
    assert!(!custom.is_non_integral_address_space(address_space::<0>()));
    assert!(!custom.is_non_integral_address_space(address_space::<1>()));
    assert!(custom.is_non_integral_address_space(two));
    assert!(custom.must_not_introduce_int_to_ptr(two));
    assert!(custom.must_not_introduce_ptr_to_int(two));
    assert!(custom.is_non_integral_address_space(top));
    assert!(custom.must_not_introduce_int_to_ptr(top));
    assert!(custom.must_not_introduce_ptr_to_int(top));

    // Pointers are marked as non-integral if the address size != total size
    for layout in ["p2:64:64:64:32", "p2:128:64:64:64"] {
        let dl = parse(layout);
        assert!(dl.is_non_integral_address_space(two), "{layout}");
        assert!(!dl.has_unstable_representation(two), "{layout}");
        assert!(!dl.has_external_state(two), "{layout}");
        assert!(!dl.must_not_introduce_int_to_ptr(two), "{layout}");
        assert!(!dl.must_not_introduce_ptr_to_int(two), "{layout}");
        assert_eq!(spaces(&dl), [two], "{layout}");
    }
    // Pointers can be marked as unstable using 'pu'
    for layout in ["pu2:64:64:64:64", "pu2:64:64:64:32"] {
        let dl = parse(layout);
        // Note: isNonIntegralAddressSpace returns true for even with index ==
        assert!(dl.is_non_integral_address_space(two), "{layout}");
        assert!(dl.has_unstable_representation(two), "{layout}");
        assert!(!dl.has_external_state(two), "{layout}");
        assert!(dl.must_not_introduce_ptr_to_int(two), "{layout}");
        assert!(dl.must_not_introduce_int_to_ptr(two), "{layout}");
        assert_eq!(spaces(&dl), [two], "{layout}");
    }

    // Non-integral pointers with external state ('e' flag).
    for layout in ["pe2:64:64:64:32", "pe2:64:64:64:64"] {
        let dl = parse(layout);
        assert!(dl.is_non_integral_address_space(two), "{layout}");
        assert!(dl.has_external_state(two), "{layout}");
        assert!(dl.must_not_introduce_int_to_ptr(two), "{layout}");
        assert!(!dl.must_not_introduce_ptr_to_int(two), "{layout}");
        assert!(!dl.has_unstable_representation(two), "{layout}");
        assert_eq!(spaces(&dl), [two], "{layout}");
    }

    // It is also possible to have both unstable representation and external state
    for layout in ["peu2:64:64:64:32", "pue2:128:64:64:64"] {
        let dl = parse(layout);
        assert!(dl.is_non_integral_address_space(two), "{layout}");
        assert!(dl.has_external_state(two), "{layout}");
        assert!(custom.must_not_introduce_int_to_ptr(two), "{layout}");
        assert!(custom.must_not_introduce_ptr_to_int(two), "{layout}");
        assert!(dl.has_unstable_representation(two), "{layout}");
        assert_eq!(spaces(&dl), [two], "{layout}");
    }

    // For backwards compatibility, the ni DataLayout part overrides any
    // p[e][u].
    for layout in [
        "ni:2-p2:64:64:64:32",
        "ni:2-pu2:64:64:64:32",
        "ni:2-pu2:64:64:64:32",
        "p2:64:64:64:32-ni:2",
        "pu2:64:64:64:32-ni:2",
        "pe2:64:64:64:32-ni:2",
        "peeee2:64:64:64:32-pu2:64:64:64:32-ni:2",
    ] {
        let dl = parse(layout);
        assert!(dl.is_non_integral_address_space(two), "{layout}");
        assert!(dl.has_unstable_representation(two), "{layout}");
        // The external state property is new and not expected for existing uses of
        // non-integral pointers, so existing :ni data layouts should not set it.
        assert!(!dl.has_external_state(two), "{layout}");
        assert_eq!(spaces(&dl), [two], "{layout}");
    }
}

/// Port of `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, NonIntegralHelpers)`.
/// `mustNotIntroduceIntToPtr(Type *)` and `mustNotIntroducePtrToInt(Type *)` are the
/// `_for_type` forms, because Rust has no overload. Upstream's `Ctx` is a module
/// here, which is what mints a type.
#[test]
fn non_integral_helpers() {
    let dl = parse("p1:128:128:128:64-pu2:32:32:32:32-pu3:64:64:64:32-pe4:64:64:64:32");
    assert_eq!(
        dl.non_standard_address_spaces().collect::<Vec<_>>(),
        [
            address_space::<1>(),
            address_space::<2>(),
            address_space::<3>(),
            address_space::<4>(),
        ]
    );
    let m = module_new!("m").expect("fresh module");
    for (space, non_integral, unstable, external_state, _size) in [
        (address_space::<0>(), false, false, false, 64),
        (address_space::<1>(), true, false, false, 128),
        (address_space::<2>(), true, true, false, 32),
        (address_space::<3>(), true, true, false, 64),
        (address_space::<4>(), true, false, true, 64),
    ] {
        assert_eq!(non_integral, dl.is_non_integral_address_space(space));
        assert_eq!(unstable, dl.has_unstable_representation(space));
        assert_eq!(external_state, dl.has_external_state(space));
        let avoid_int_to_ptr = unstable || external_state;
        assert_eq!(avoid_int_to_ptr, dl.must_not_introduce_int_to_ptr(space));
        let avoid_ptr_to_int = unstable;
        assert_eq!(avoid_ptr_to_int, dl.must_not_introduce_ptr_to_int(space));
        let ptr_ty = m.ptr_type(space);
        for ty in [
            ptr_ty.as_type(),
            VectorType::of(ptr_ty, lanes::<2>()).as_type(),
            VectorType::scalable_of(ptr_ty, lanes::<1>()).as_type(),
        ] {
            assert_eq!(
                avoid_ptr_to_int,
                dl.must_not_introduce_ptr_to_int_for_type(ty)
            );
            assert_eq!(
                avoid_int_to_ptr,
                dl.must_not_introduce_int_to_ptr_for_type(ty)
            );
            // The old API should return true for both unstable and non-integral.
            assert_eq!(
                unstable || non_integral,
                dl.is_non_integral_pointer_type(ty)
            );
        }
        // Both helpers gracefully handle non-pointer, non-vector-of-pointers:
        let i1_ty = m.bool_type().as_type();
        assert!(!dl.must_not_introduce_ptr_to_int_for_type(i1_ty));
        assert!(!dl.must_not_introduce_int_to_ptr_for_type(i1_ty));
    }
}

// ---------------------------------------------------------------------------
// Round-trip via Display
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `Module::setDataLayout(StringRef)` /
/// `getDataLayout().getStringRepresentation()` in
/// `lib/IR/AsmWriter.cpp::printModule` (the `target datalayout` line
/// is emitted from the unparsed string). Asserts the typical
/// x86_64-linux layout round-trips byte-stable.
#[test]
fn x86_64_linux_round_trip() {
    let s = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128";
    let dl = parse(s);
    assert_eq!(format!("{dl}"), s);
    assert_eq!(dl.string_representation(), s);
    assert!(dl.is_little_endian());
    assert_eq!(dl.mangling_mode(), ManglingMode::Elf);
    assert_eq!(dl.pointer_size_in_bits(address_space::<0>()), 64);
    assert_eq!(dl.pointer_size_in_bits(address_space::<270>()), 32);
    assert!(dl.is_legal_integer(64));
    assert_eq!(dl.stack_alignment(), Some(Align::new(16).expect("a")));
}

/// `llvmkit-specific`: arm64-darwin round-trip. Layout copied from
/// `clang::TargetInfo::DataLayoutString` for the `aarch64-apple-darwin`
/// triple; mirrors the role of upstream's `Triple` -> layout-string
/// mapping in `lib/Target/AArch64/AArch64TargetMachine.cpp`.
#[test]
fn aarch64_darwin_round_trip() {
    let s = "e-m:o-i64:64-i128:128-n32:64-S128";
    let dl = parse(s);
    assert_eq!(format!("{dl}"), s);
    assert_eq!(dl.mangling_mode(), ManglingMode::MachO);
}

/// `llvmkit-specific`: wasm32 round-trip. Layout from
/// `lib/Target/WebAssembly/WebAssemblyTargetMachine.cpp`.
#[test]
fn wasm32_round_trip() {
    let s = "e-m:e-p:32:32-p10:8:8-p20:8:8-i64:64-n32:64-S128-ni:1:10:20";
    let dl = parse(s);
    assert_eq!(format!("{dl}"), s);
    assert_eq!(dl.pointer_size_in_bits(address_space::<0>()), 32);
    assert!(dl.is_non_integral_address_space(address_space::<1>()));
    assert!(dl.is_non_integral_address_space(address_space::<10>()));
    assert!(dl.is_non_integral_address_space(address_space::<20>()));
}

// ---------------------------------------------------------------------------
// Module wiring
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `Module::setDataLayout` /
/// `Module::getDataLayout` in `IR/Module.h`. Asserts that the
/// AsmWriter emits the `target datalayout` directive when the module
/// has a non-default layout.
#[test]
fn module_emits_target_datalayout_directive() {
    let m = module_new!("m").expect("fresh module");
    m.set_data_layout(DataLayout::parse("e-m:e-p:64:64-i64:64-n8:16:32:64-S128").expect("parse"));
    let text = format!("{m}");
    assert_line(
        &text,
        "target datalayout = \"e-m:e-p:64:64-i64:64-n8:16:32:64-S128\"",
    );
}

/// `llvmkit-specific`: mirrors `Module::setTargetTriple` /
/// `Module::getTargetTriple`. Asserts emission of the directive.
#[test]
fn module_emits_target_triple_directive() {
    let m = module_new!("m").expect("fresh module");
    m.set_target_triple("x86_64-pc-linux-gnu");
    let text = format!("{m}");
    assert_line(&text, "target triple = \"x86_64-pc-linux-gnu\"");
    m.clear_target_triple();
    assert!(m.target_triple().is_none());
    assert!(!format!("{m}").contains("target triple"));
}

/// `llvmkit-specific`: mirrors `Module::setModuleInlineAsm` and the
/// `do { ... } while (!Asm.empty())` loop in
/// `lib/IR/AsmWriter.cpp::printModule`.
#[test]
fn module_emits_module_asm_directive() {
    let m = module_new!("m").expect("fresh module");
    m.set_module_asm("beep boop");
    let text = format!("{m}");
    assert_line(&text, "module asm \"beep boop\"");
}

// ---------------------------------------------------------------------------
// Type-size accessors against the default layout
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `DataLayout::getTypeSizeInBits` arms
/// in the inline definition in `IR/DataLayout.h` (per-type case
/// table).
#[test]
fn type_size_in_bits_basic_types() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    for (ty, bits) in [
        (SizedType::from(m.bool_type()), 1),
        (SizedType::from(m.i8_type()), 8),
        (SizedType::from(m.i32_type()), 32),
        (SizedType::from(m.i64_type()), 64),
        (SizedType::from(m.half_type()), 16),
        (SizedType::from(m.f32_type()), 32),
        (SizedType::from(m.f64_type()), 64),
        (SizedType::from(m.fp128_type()), 128),
        (SizedType::from(m.x86_fp80_type()), 80),
    ] {
        assert_eq!(dl.type_size_in_bits(ty), TypeSize::fixed(bits), "{ty}");
    }
}

/// `llvmkit-specific`: mirrors `DataLayout::getTypeStoreSize` for
/// non-power-of-two integers (i36 -> 5 bytes, x86_fp80 -> 10 bytes).
#[test]
fn type_store_size_non_power_of_two() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let i36 = m.custom_width_int_type(36).expect("i36");
    assert_eq!(dl.type_store_size(i36), TypeSize::fixed(5));
    assert_eq!(dl.type_store_size(m.x86_fp80_type()), TypeSize::fixed(10));
}

/// `llvmkit-specific`: mirrors `DataLayout::getTypeAllocSize` for the
/// `i64:32:64` int spec arm: alloc-size includes ABI alignment
/// padding.
#[test]
fn type_alloc_size_i64_default() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    // Default i64 spec is `i64:32:64` (ABI=4, pref=8). So an i64
    // alloc-size rounds 8 up to 4-byte alignment -- still 8.
    assert_eq!(dl.type_alloc_size(m.i64_type()), TypeSize::fixed(8));
}

/// `llvmkit-specific`: mirrors `DataLayout::getABITypeAlign` for
/// integers using the default `i32:32:32` spec.
#[test]
fn abi_type_align_i32_default() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    assert_eq!(dl.abi_type_align(m.i32_type()), Align::new(4).expect("a"));
    assert_eq!(dl.abi_type_align(m.f64_type()), Align::new(8).expect("a"));
}

// ---------------------------------------------------------------------------
// Struct layout
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `DataLayout::getStructLayout` /
/// `StructLayout::StructLayout` for a non-packed `{i32, i64}` struct
/// against the default layout. Field 0 at offset 0, field 1 at
/// offset 8 (aligned past the i64 ABI alignment of 4 -- but with
/// the default i64 having pref=8, ABI=4, the placement is aligned
/// to 4).
#[test]
fn struct_layout_simple() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let i32_ty = m.i32_type();
    let i64_ty = m.i64_type();
    let st = m
        .struct_type([i32_ty.as_type(), i64_ty.as_type()])
        .expect("valid struct elements");
    let layout = dl.struct_layout(st).expect("a struct with no opaque field");
    assert_eq!(layout.element_offset(0), TypeSize::fixed(0));
    // i64 default ABI alignment is 4, so i64 placed at offset 4.
    assert_eq!(layout.element_offset(1), TypeSize::fixed(4));
    assert_eq!(layout.size_in_bytes(), TypeSize::fixed(12));
}

/// `llvmkit-specific`: mirrors `StructLayout::StructLayout` packed
/// arm: every field has alignment 1.
#[test]
fn struct_layout_packed() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let i8_ty = m.i8_type();
    let i32_ty = m.i32_type();
    let st = m
        .packed_struct_type([i8_ty.as_type(), i32_ty.as_type()])
        .expect("valid struct elements");
    let layout = dl.struct_layout(st).expect("a struct with no opaque field");
    assert_eq!(layout.element_offset(0), TypeSize::fixed(0));
    assert_eq!(layout.element_offset(1), TypeSize::fixed(1));
    assert_eq!(layout.size_in_bytes(), TypeSize::fixed(5));
}

/// Mirrors `StructLayout::getElementOffset`, a `TypeSize`, and
/// `StructLayout::getElementContainingOffset`. The upstream comment there is the
/// fixed case: "in { i32, [0 x i32], i32 }, searching for offset 4 will stop at
/// the i32 in the end". The scalable half has **no upstream counterpart**: upstream
/// asserts "Cannot get element at offset for structure containing scalable
/// vector types", and llvmkit answers `None`. A field of a scalable struct has
/// a scalable offset, so reading it as a number takes a decision.
#[test]
fn a_struct_field_offset_says_whether_it_scales() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let i32_ty = m.i32_type();
    let fixed = m
        .struct_type([
            i32_ty.as_type(),
            ArrayType::of(i32_ty, 0).as_type(),
            i32_ty.as_type(),
        ])
        .expect("valid struct elements");
    let fixed_layout = dl
        .struct_layout(fixed)
        .expect("a struct with no opaque field");
    assert_eq!(fixed_layout.element_containing_offset(4), Some(2));
    assert_eq!(fixed_layout.element_containing_offset(0), Some(0));
    assert_eq!(fixed_layout.element_containing_offset(8), None);

    let scalable_field = VectorType::scalable_of(i32_ty, lanes::<4>()).as_type();
    let scalable = m
        .struct_type([scalable_field, scalable_field])
        .expect("valid struct elements");
    let scalable_layout = dl
        .struct_layout(scalable)
        .expect("a struct with no opaque field");
    assert_eq!(scalable_layout.element_offset(1), TypeSize::scalable(16));
    assert_eq!(
        scalable_layout.element_offset_in_bits(1),
        TypeSize::scalable(128)
    );
    assert_eq!(scalable_layout.element_containing_offset(4), None);
}

/// Mirrors `DataLayout::getTypeSizeInBits`, `getTypeAllocSize` and the
/// `StructLayout` constructor: scalability survives an array's multiplication
/// (`operator*`) and a homogeneous struct's sum (`if (i == 0 &&
/// Ty->isScalableTy()) StructSize = TypeSize::getScalable(0)`, then
/// `operator+=`). llvmkit-specific (rule anchor): no upstream test reads either.
#[test]
fn a_scalable_size_stays_scalable_through_an_array_and_a_struct() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let sized = |ty| SizedType::try_from(ty).expect("built of a vector of integers");
    let vector = VectorType::scalable_of(m.i32_type(), lanes::<4>()).as_type();
    assert_eq!(dl.type_size_in_bits(sized(vector)), TypeSize::scalable(128));

    let array = sized(m.array_type(vector, 2).expect("a valid element").as_type());
    assert_eq!(dl.type_size_in_bits(array), TypeSize::scalable(256));
    assert_eq!(dl.type_alloc_size(array), TypeSize::scalable(32));
    assert_eq!(dl.type_store_size(array), TypeSize::scalable(32));

    let tuple_struct = m
        .struct_type([vector, vector])
        .expect("valid struct elements");
    let tuple = sized(tuple_struct.as_type());
    assert_eq!(dl.type_size_in_bits(tuple), TypeSize::scalable(256));
    assert_eq!(dl.type_alloc_size(tuple), TypeSize::scalable(32));
    let layout = dl
        .struct_layout(tuple_struct)
        .expect("a struct with no opaque field");
    assert_eq!(layout.size_in_bytes(), TypeSize::scalable(32));
    assert_eq!(layout.size_in_bits(), TypeSize::scalable(256));
    assert!(!layout.is_padded());

    // `operator+=` alone takes a non-zero field's scalability. A first field of
    // size zero is what `getScalable(0)` is for.
    let empty_array = m.array_type(vector, 0).expect("a valid element").as_type();
    let empty_tuple = m.struct_type([empty_array]).expect("valid struct elements");
    assert_eq!(
        dl.struct_layout(empty_tuple)
            .expect("a struct with no opaque field")
            .size_in_bytes(),
        TypeSize::scalable(0)
    );

    let i64_array = m.array_type(m.i64_type(), 2).expect("a valid element");
    assert_eq!(
        dl.type_size_in_bits(sized(i64_array.as_type())),
        TypeSize::fixed(128)
    );
}

/// `StructLayout::StructLayout` adds each field's alloc size with
/// `TypeSize::operator+=`, which asserts "Incompatible types" when a nonzero
/// scalable size meets a nonzero fixed one, so `{ <vscale x 4 x i32>, i8 }`
/// and `{ i8, <vscale x 4 x i32> }` have no layout upstream. **No upstream
/// test** reaches the assert; llvmkit answers `None` there
/// (`docs/divergences.md` D43). llvmkit-specific regression: the sum took
/// the flag of its right side, so the first struct was laid out with a
/// scalable offset and a fixed 32-byte size. A zero-sized fixed field passes
/// the assert, as it passes upstream's: `{ <vscale x 4 x i32>, [0 x i8] }`
/// is laid out, at a scalable 16 bytes, though the struct is unsized.
#[test]
fn a_struct_mixing_scalable_and_fixed_sizes_has_no_layout() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let scalable = VectorType::scalable_of(m.i32_type(), lanes::<4>()).as_type();
    let i8_ty = m.i8_type().as_type();
    for fields in [[scalable, i8_ty], [i8_ty, scalable]] {
        let mixed = m.struct_type(fields).expect("valid struct elements");
        assert!(dl.struct_layout(mixed).is_none(), "{mixed}");
    }
    let empty = m
        .array_type(m.i8_type(), 0)
        .expect("a valid element")
        .as_type();
    let zero_tail = m
        .struct_type([scalable, empty])
        .expect("valid struct elements");
    assert!(!zero_tail.as_type().is_sized());
    assert_eq!(
        dl.struct_layout(zero_tail)
            .expect("a zero-sized field adds nothing")
            .size_in_bytes(),
        TypeSize::scalable(16)
    );
}

/// `StructLayout::StructLayout` asks a field's `getABITypeAlign`, which
/// asserts `isSized`, only for a non-packed struct; a packed one asks only
/// `getTypeAllocSize`, which in `DataLayout.cpp` asks no `isSized` of a
/// struct or an array, only that the struct has a layout. So an unsized
/// struct with a layout, `{ <vscale x 4 x i32>, <vscale x 8 x i16> }` (two
/// scalable vectors of different types, so not homogeneous), is a valid
/// field of a packed struct and of an array in one, and not of a non-packed
/// struct. A packed struct's opaque field, or one of a target extension type
/// with no layout, still asserts (`getStructLayout`'s `!isOpaque()`, and
/// `getTypeStoreSize`'s `isSized`). **No upstream test**; llvmkit-specific
/// regression: `struct_layout` refused every unsized field, packed or not,
/// rejecting structs upstream lays out.
#[test]
fn a_packed_struct_lays_out_an_unsized_field_that_has_an_alloc_size() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let i32x4 = VectorType::scalable_of(m.i32_type(), lanes::<4>()).as_type();
    let i16x8 = VectorType::scalable_of(m.i16_type(), lanes::<8>()).as_type();
    let mixed = m
        .struct_type([i32x4, i16x8])
        .expect("valid struct elements");
    assert!(
        !mixed.as_type().is_sized(),
        "two scalable types, not homogeneous"
    );
    assert_eq!(
        dl.struct_layout(mixed)
            .expect("an unsized struct with a layout")
            .size_in_bytes(),
        TypeSize::scalable(32)
    );
    let array = m.array_type(mixed, 1).expect("a valid element").as_type();
    for field in [mixed.as_type(), array] {
        let packed = m
            .packed_struct_type([field])
            .expect("valid struct elements");
        assert_eq!(
            dl.struct_layout(packed)
                .expect("a packed struct asks no field's alignment")
                .size_in_bytes(),
            TypeSize::scalable(32),
            "{packed}"
        );
        let unpacked = m.struct_type([field]).expect("valid struct elements");
        assert!(dl.struct_layout(unpacked).is_none(), "{unpacked}");
    }
    let opaque = m.opaque_struct("O").expect("fresh name").as_type();
    let no_layout = m
        .target_ext_type("spirv.Padding", Vec::<Type<'_, _>>::new(), Vec::new())
        .expect("`checkParams` admits it")
        .as_type();
    for field in [opaque, no_layout] {
        let packed = m
            .packed_struct_type([field])
            .expect("valid struct elements");
        assert!(dl.struct_layout(packed).is_none(), "{packed}");
    }
}

// ---------------------------------------------------------------------------
// Defaults / isDefault
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `DataLayout::isDefault`.
#[test]
fn default_layout_is_default() {
    let dl = DataLayout::default();
    assert!(dl.is_default());
    assert!(dl.string_representation().is_empty());
    let parsed = parse("e");
    assert!(!parsed.is_default());
}

/// Mirrors `DataLayout::getValueOrABITypeAlignment`, `Alignment ?
/// *Alignment : getABITypeAlign(Ty)`: a set alignment is answered as is, an
/// unset one is the type's ABI alignment. The type is read only when the
/// alignment is unset, so a set alignment is answered for an unsized type
/// too. The last case has **no upstream counterpart**: an unset alignment on
/// an unsized type meets `getABITypeAlign`'s `isSized` assert, refused here
/// as `IrError::UnsizedType` (`docs/divergences.md` D43).
#[test]
fn value_or_abi_type_align() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let custom = MaybeAlign::from(Align::new(16).expect("a"));
    let i32_ty = m.i32_type();
    assert_eq!(
        dl.value_or_abi_type_align(custom, i32_ty),
        Ok(Align::new(16).expect("a"))
    );
    assert_eq!(
        dl.value_or_abi_type_align(MaybeAlign::default(), i32_ty),
        Ok(Align::new(4).expect("a"))
    );
    let opaque = m.opaque_struct("O").expect("fresh name").as_type();
    assert_eq!(
        dl.value_or_abi_type_align(custom, opaque),
        Ok(Align::new(16).expect("a"))
    );
    assert_eq!(
        dl.value_or_abi_type_align(MaybeAlign::default(), opaque),
        Err(IrError::UnsizedType {
            kind: TypeKindLabel::Struct
        })
    );
}

/// `llvmkit-specific`: `FromStr` is `DataLayout::parse` under the trait, so
/// `"…".parse()` and the named entry point agree — including on the error,
/// which stays [`DataLayoutError`] with its specific reason rather than a
/// generic keyword rejection. No upstream counterpart: C++ has no `FromStr`;
/// the functional reference is
/// `static Expected<DataLayout> DataLayout::parse(StringRef)`
/// (`lib/IR/DataLayout.cpp`).
#[test]
fn from_str_agrees_with_parse() {
    let text = "e-p:64:64:64-i64:64:64-n8:16:32:64-S128";
    let via_trait: DataLayout = text.parse().expect("layout parses");
    assert_eq!(via_trait, parse(text));
    assert_eq!(via_trait.to_string(), text);

    match "e-p:0:64".parse::<DataLayout>() {
        Err(DataLayoutError { reason }) => {
            assert_eq!(reason, parse_err("e-p:0:64"));
        }
        Ok(dl) => panic!("expected an error, got {dl:?}"),
    }
}

/// `llvmkit-specific`: a layout is a natural cache key, so `Hash` must agree
/// with the `Eq` it ships beside — equal layouts hash equal, and the parsed
/// and default forms of the same string are the same key. No upstream
/// counterpart: `llvm::DataLayout` has no hash; the functional reference is
/// its `operator==` (`include/llvm/IR/DataLayout.h`).
#[test]
fn equal_layouts_hash_equal() {
    use std::collections::HashMap;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(dl: &DataLayout) -> u64 {
        let mut hasher = DefaultHasher::new();
        dl.hash(&mut hasher);
        hasher.finish()
    }

    let a = parse("e-p:64:64:64-i64:64:64");
    let b = parse("e-p:64:64:64-i64:64:64");
    let other = parse("E-p:64:64:64-i64:64:64");
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
    assert_ne!(a, other);

    let mut cache: HashMap<DataLayout, u32> = HashMap::new();
    cache.insert(a.clone(), 1);
    assert_eq!(cache.get(&b), Some(&1));
    assert_eq!(cache.get(&other), None);
}

/// `llvmkit-specific` (`docs/divergences.md` D43): a struct holding an
/// opaque struct has no `SizedType` proof, so no size query can be asked of
/// it, and no `struct_layout`, where upstream asserts (`isSized` in
/// `getTypeSizeInBits` / `getAlignment`, and `!ST->isOpaque()` in
/// `StructLayout::StructLayout`). Nothing is cached for it either, so once
/// the opaque struct gets a body the size and layout are the real ones. A
/// cached layout would go stale here, and a panic (the `unreachable!` an
/// earlier version had) would abort a public query.
#[test]
fn struct_layout_of_an_opaque_field_is_refused_and_not_cached() {
    let m = module_new!("m").expect("fresh module");
    let dl = DataLayout::default();
    let opaque = m.opaque_struct("O").expect("fresh name");
    let outer = m
        .struct_type([m.i8_type().as_type(), opaque.as_type()])
        .expect("valid struct elements");
    assert!(matches!(
        SizedType::try_from(outer.as_type()),
        Err(IrError::UnsizedType { .. })
    ));
    assert!(
        dl.struct_layout(outer).is_none(),
        "a struct holding an opaque struct has no layout"
    );
    m.set_struct_body(opaque, [m.i32_type().as_type()], false)
        .expect("the body is set once");
    let outer_sized = SizedType::try_from(outer.as_type()).expect("sized once the body is set");
    assert_eq!(dl.type_alloc_size(outer_sized), TypeSize::fixed(8));
    assert_eq!(
        dl.struct_layout(outer)
            .expect("a struct with no opaque field")
            .element_offset(1),
        TypeSize::fixed(4)
    );
}

/// Mirrors `TEST(DataLayoutTest, Equality)`: `p00(global):32:8` and
/// `p0(global):32:8` keep their own strings and compare equal, since
/// `DataLayout::operator==` does not read the string. Beyond the port, the
/// two hash equal, which llvmkit's `Hash` owes its `Eq`.
#[test]
fn equality() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let layout0 = "p00(global):32:8";
    let layout1 = "p0(global):32:8";
    let dl0 = parse(layout0);
    let dl1 = parse(layout1);
    assert_eq!(dl0.string_representation(), layout0);
    assert_eq!(dl1.string_representation(), layout1);
    assert_eq!(dl0, dl1);
    let hash_of = |dl: &DataLayout| {
        let mut hasher = DefaultHasher::new();
        dl.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash_of(&dl0), hash_of(&dl1));
}

/// Mirrors `TEST(DataLayoutTest, CopyAssignmentInvalidatesStructLayout)`:
/// each `DataLayout` lays a struct out under its own pointer size, and a copy
/// of the second, assigned over the first, answers the second's layout, not
/// the first's cached one. Upstream's `operator=` deletes the target's
/// `StructLayoutMap`; llvmkit's clone starts with an empty one.
#[test]
fn copy_assignment_invalidates_struct_layout() {
    let mut dl1 = parse("p:32:32");
    let dl2 = parse("p:64:64");
    let m = module_new!("m").expect("fresh module");
    let ty = m
        .struct_type([m.ptr_type_in::<0>().as_type()])
        .expect("valid struct elements");
    let layout = |dl: &DataLayout| dl.struct_layout(ty).expect("a pointer struct has a layout");

    // Initialize struct layout caches.
    assert_eq!(layout(&dl1).size_in_bits(), TypeSize::fixed(32));
    assert_eq!(
        layout(&dl1).alignment(),
        Align::new(4).expect("a power of two")
    );
    assert_eq!(layout(&dl2).size_in_bits(), TypeSize::fixed(64));
    assert_eq!(
        layout(&dl2).alignment(),
        Align::new(8).expect("a power of two")
    );

    // The copy should invalidate DL1's cache.
    dl1 = dl2.clone();
    assert_eq!(layout(&dl1).size_in_bits(), TypeSize::fixed(64));
    assert_eq!(
        layout(&dl1).alignment(),
        Align::new(8).expect("a power of two")
    );
    assert_eq!(layout(&dl2).size_in_bits(), TypeSize::fixed(64));
    assert_eq!(
        layout(&dl2).alignment(),
        Align::new(8).expect("a power of two")
    );
}

/// llvmkit-specific, no upstream counterpart: every type that gets a
/// [`SizedType`] proof answers every `DataLayout` size and alignment query
/// without reaching an unsized type. The walks under the queries treat an
/// unsized type as unreachable (`reached_unsized`), which holds only while
/// `Type::is_sized` (`StructType::isSized`, `Type::isSizedDerivedType`), the
/// target extension layout table, `struct_layout_inner`'s refusals and
/// `alloc_size_defined` agree; a disagreement aborts a public query. So the
/// test also pins that a sized struct has a layout (`layout_of` relies on
/// it) and that a non-packed struct with an unsized field has none. The
/// types here are every leaf kind, the target extension types
/// `getTargetTypeInfo` lays out and the ones it cannot, two levels of array,
/// vector, scalable vector, struct and packed struct nesting over them, and
/// the same again after the opaque struct among them gets a body. Each type
/// without a proof is refused with
/// `IrError::UnsizedType` by `value_or_abi_type_align`'s unset branch. The
/// counts are a floor on coverage, not an upstream figure: both sides must
/// be exercised.
#[test]
fn every_sized_type_answers_every_size_query() {
    let m = module_new!("m").expect("fresh module");
    let i8_ty = m.i8_type().as_type();
    let i32_ty = m.i32_type();
    let opaque = m.opaque_struct("O").expect("fresh name");
    let scalable_i8 = VectorType::scalable_of(m.i8_type(), lanes::<8>()).as_type();
    let mut leaves: Vec<Type<'_, _>> = vec![
        m.bool_type().as_type(),
        i8_ty,
        i32_ty.as_type(),
        m.i64_type().as_type(),
        m.custom_width_int_type(36).expect("i36").as_type(),
        m.half_type().as_type(),
        m.bfloat_type().as_type(),
        m.f32_type().as_type(),
        m.f64_type().as_type(),
        m.x86_fp80_type().as_type(),
        m.fp128_type().as_type(),
        m.ppc_fp128_type().as_type(),
        m.x86_amx_type(),
        m.wasm_exnref_type(),
        m.ptr_type_in::<0>().as_type(),
        m.ptr_type_in::<3>().as_type(),
        m.void_type().as_type(),
        m.label_type().as_type(),
        m.metadata_type().as_type(),
        m.token_type().as_type(),
        m.function_type(m.void_type(), [i32_ty.as_type()])
            .expect("a valid function type")
            .as_type(),
        VectorType::scalable_of(i32_ty, lanes::<4>()).as_type(),
        opaque.as_type(),
    ];
    let no_types = Vec::<Type<'_, _>>::new();
    for (name, types, ints) in [
        ("spirv.Type", no_types.clone(), vec![0, 4]),
        ("spirv.Type", no_types.clone(), vec![0, 4, 32]),
        ("spirv.Padding", no_types.clone(), vec![]),
        ("spirv.Padding", no_types.clone(), vec![4]),
        ("dx.Padding", no_types.clone(), vec![4]),
        ("riscv.vector.tuple", vec![i32_ty.as_type()], vec![2]),
        ("riscv.vector.tuple", vec![scalable_i8], vec![2]),
        ("aarch64.svcount", no_types.clone(), vec![]),
        ("llvm.test.vectorelement", no_types.clone(), vec![]),
        ("amdgcn.named.barrier", no_types.clone(), vec![0]),
    ] {
        leaves.push(
            m.target_ext_type(name, types, ints)
                .expect("`checkParams` admits it")
                .as_type(),
        );
    }
    let mut all = leaves;
    for _ in 0..2 {
        let mut next = Vec::new();
        for &ty in &all {
            next.extend(m.array_type(ty, 3).ok().map(|t| t.as_type()));
            next.extend(m.vector_type(ty, lanes::<2>()).ok().map(|t| t.as_type()));
            next.extend(
                m.scalable_vector_type(ty, lanes::<2>())
                    .ok()
                    .map(|t| t.as_type()),
            );
            next.extend(m.struct_type([ty]).ok().map(|t| t.as_type()));
            next.extend(m.struct_type([ty, ty]).ok().map(|t| t.as_type()));
            next.extend(m.struct_type([ty, i8_ty]).ok().map(|t| t.as_type()));
            next.extend(m.packed_struct_type([ty]).ok().map(|t| t.as_type()));
            next.extend(m.packed_struct_type([ty, i8_ty]).ok().map(|t| t.as_type()));
        }
        all.extend(next);
    }

    let check = |all: &[Type<'_, _>]| {
        let dl = DataLayout::default();
        let (mut sized_count, mut unsized_count) = (0_usize, 0_usize);
        for &ty in all {
            if let Ok(struct_ty) = StructType::try_from(ty) {
                // Laying a struct out, packed or not, reaches no type the
                // walks hold unreachable either; a sized struct has a layout,
                // and a non-packed one with an unsized field (or an opaque
                // body) has none, where `StructLayout`'s `getABITypeAlign`
                // asserts. A packed struct asks no field's alignment, so an
                // unsized field with an alloc size is laid out.
                let layout = dl.struct_layout(struct_ty);
                if ty.is_sized() {
                    assert!(layout.is_some(), "{ty}: a sized struct has a layout");
                }
                let has_unsized_field = struct_ty.is_opaque()
                    || (!struct_ty.is_packed()
                        && struct_ty.fields().any(|field| !field.is_sized()));
                if has_unsized_field {
                    assert!(layout.is_none(), "{ty}: an unsized field has no layout");
                }
            }
            let Ok(sized) = SizedType::try_from(ty) else {
                unsized_count += 1;
                assert!(!ty.is_sized(), "{ty}");
                assert!(
                    matches!(
                        dl.value_or_abi_type_align(MaybeAlign::default(), ty),
                        Err(IrError::UnsizedType { .. })
                    ),
                    "{ty}"
                );
                continue;
            };
            sized_count += 1;
            let bits = dl.type_size_in_bits(sized);
            let store = dl.type_store_size(sized);
            let alloc = dl.type_alloc_size(sized);
            assert_eq!(bits.is_scalable(), alloc.is_scalable(), "{ty}");
            assert!(
                alloc.known_min_value() >= store.known_min_value(),
                "{ty}: alloc {alloc:?} < store {store:?}"
            );
            let _ = dl.type_store_size_in_bits(sized);
            let _ = dl.type_alloc_size_in_bits(sized);
            let _ = dl.type_size_equals_store_size(sized);
            let abi = dl.abi_type_align(sized);
            assert!(dl.pref_type_align(sized) >= abi, "{ty}");
            assert_eq!(
                dl.value_or_abi_type_align(MaybeAlign::default(), ty),
                Ok(abi),
                "{ty}"
            );
        }
        assert!(sized_count > 0 && unsized_count > 0);
        (sized_count, unsized_count)
    };
    let (sized_before, unsized_before) = check(&all);
    m.set_struct_body(opaque, [i32_ty.as_type()], false)
        .expect("the body is set once");
    let (sized_after, unsized_after) = check(&all);
    assert!(
        sized_after > sized_before && unsized_after < unsized_before,
        "the body makes the structs over `%O` sized"
    );
}
