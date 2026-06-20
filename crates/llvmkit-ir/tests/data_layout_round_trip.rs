//! [`DataLayout`] parser + accessor tests, ported from
//! `unittests/IR/DataLayoutTest.cpp`.
//!
//! ## Upstream provenance
//!
//! Each test cites a specific `TEST(DataLayout*, ...)` block. The Task 7
//! audited diagnostics below compare llvmkit's exact
//! [`IrError::InvalidDataLayout`] reason string; rows are marked
//! `llvmkit-specific subset` when that string intentionally differs from
//! upstream `FailedWithMessage` text.

use llvmkit_ir::{Align, DataLayout, IrError, ManglingMode, MaybeAlign, Module};

fn parse(s: &str) -> DataLayout {
    DataLayout::parse(s).unwrap_or_else(|e| panic!("parse {s:?}: {e:?}"))
}

fn parse_err(s: &str) -> String {
    match DataLayout::parse(s) {
        Ok(_) => panic!("expected error for {s:?}"),
        Err(IrError::InvalidDataLayout { reason }) => reason,
        Err(other) => panic!("expected InvalidDataLayout, got {other:?}"),
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
        assert_eq!(dl.pointer_size_in_bits(0), *v0, "{s} AS 0");
        assert_eq!(dl.pointer_size_in_bits(1), *v1, "{s} AS 1");
        assert_eq!(dl.pointer_size_in_bits(2), *v2, "{s} AS 2");
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
        assert_eq!(dl.pointer_size(0), *v0, "{s} AS 0");
        assert_eq!(dl.pointer_size(1), *v1, "{s} AS 1");
        assert_eq!(dl.pointer_size(2), *v2, "{s} AS 2");
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
        assert_eq!(dl.index_size_in_bits(0), *v0, "{s} AS 0");
        assert_eq!(dl.index_size_in_bits(1), *v1, "{s} AS 1");
        assert_eq!(dl.index_size_in_bits(2), *v2, "{s} AS 2");
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
        assert_eq!(dl.index_size(0), *v0, "{s} AS 0");
        assert_eq!(dl.index_size(1), *v1, "{s} AS 1");
        assert_eq!(dl.index_size(2), *v2, "{s} AS 2");
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
        assert_eq!(dl.pointer_abi_align(0).value(), *v0, "{s} AS 0");
        assert_eq!(dl.pointer_abi_align(1).value(), *v1, "{s} AS 1");
        assert_eq!(dl.pointer_abi_align(2).value(), *v2, "{s} AS 2");
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
        assert_eq!(dl.pointer_pref_align(0).value(), *v0, "{s} AS 0");
        assert_eq!(dl.pointer_pref_align(1).value(), *v1, "{s} AS 1");
        assert_eq!(dl.pointer_pref_align(2).value(), *v2, "{s} AS 2");
    }
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, AddressSpaceName)`.
#[test]
fn address_space_name() {
    let dl = parse("p:16:32-p1(foo):16:32-p10(bar):16:16");
    assert_eq!(dl.address_space_name(0), "");
    assert_eq!(dl.address_space_name(1), "foo");
    assert_eq!(dl.address_space_name(10), "bar");
    assert_eq!(dl.address_space_name(3), "");
    assert_eq!(dl.named_address_space("foo"), Some(1));
    assert_eq!(dl.named_address_space("bar"), Some(10));
    assert_eq!(dl.named_address_space("missing"), None);
}

/// Mirrors `unittests/IR/DataLayoutTest.cpp::TEST(DataLayout, IsNonIntegralAddressSpace)`.
#[test]
fn is_non_integral_address_space() {
    let default = DataLayout::default();
    assert!(default.non_standard_address_spaces().is_empty());
    assert!(!default.is_non_integral_address_space(0));
    assert!(!default.is_non_integral_address_space(1));

    let custom = parse("ni:2:16777215");
    assert_eq!(
        custom.non_integral_address_spaces(),
        vec![2u32, 16777215u32]
    );
    assert!(!custom.is_non_integral_address_space(0));
    assert!(!custom.is_non_integral_address_space(1));
    assert!(custom.is_non_integral_address_space(2));
    assert!(custom.is_non_integral_address_space(16777215));
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
    assert_eq!(dl.pointer_size_in_bits(0), 64);
    assert_eq!(dl.pointer_size_in_bits(270), 32);
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
    assert_eq!(dl.pointer_size_in_bits(0), 32);
    assert!(dl.is_non_integral_address_space(1));
    assert!(dl.is_non_integral_address_space(10));
    assert!(dl.is_non_integral_address_space(20));
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
    Module::with_new("m", |m| {
        m.set_data_layout("e-m:e-p:64:64-i64:64-n8:16:32:64-S128")
            .expect("parse");
        let text = format!("{m}");
        assert_line(
            &text,
            "target datalayout = \"e-m:e-p:64:64-i64:64-n8:16:32:64-S128\"",
        );
    })
}

/// `llvmkit-specific`: mirrors `Module::setTargetTriple` /
/// `Module::getTargetTriple`. Asserts emission of the directive.
#[test]
fn module_emits_target_triple_directive() {
    Module::with_new("m", |m| {
        m.set_target_triple(Some("x86_64-pc-linux-gnu"));
        let text = format!("{m}");
        assert_line(&text, "target triple = \"x86_64-pc-linux-gnu\"");
    })
}

/// `llvmkit-specific`: mirrors `Module::setModuleInlineAsm` and the
/// `do { ... } while (!Asm.empty())` loop in
/// `lib/IR/AsmWriter.cpp::printModule`.
#[test]
fn module_emits_module_asm_directive() {
    Module::with_new("m", |m| {
        m.set_module_asm("beep boop");
        let text = format!("{m}");
        assert_line(&text, "module asm \"beep boop\"");
    })
}

// ---------------------------------------------------------------------------
// Type-size accessors against the default layout
// ---------------------------------------------------------------------------

/// `llvmkit-specific`: mirrors `DataLayout::getTypeSizeInBits` arms
/// in the inline definition in `IR/DataLayout.h` (per-type case
/// table).
#[test]
fn type_size_in_bits_basic_types() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        assert_eq!(dl.type_size_in_bits(m.bool_type().as_type()), 1);
        assert_eq!(dl.type_size_in_bits(m.i8_type().as_type()), 8);
        assert_eq!(dl.type_size_in_bits(m.i32_type().as_type()), 32);
        assert_eq!(dl.type_size_in_bits(m.i64_type().as_type()), 64);
        assert_eq!(dl.type_size_in_bits(m.half_type().as_type()), 16);
        assert_eq!(dl.type_size_in_bits(m.f32_type().as_type()), 32);
        assert_eq!(dl.type_size_in_bits(m.f64_type().as_type()), 64);
        assert_eq!(dl.type_size_in_bits(m.fp128_type().as_type()), 128);
        assert_eq!(dl.type_size_in_bits(m.x86_fp80_type().as_type()), 80);
    })
}

/// `llvmkit-specific`: mirrors `DataLayout::getTypeStoreSize` for
/// non-power-of-two integers (i36 -> 5 bytes, x86_fp80 -> 10 bytes).
#[test]
fn type_store_size_non_power_of_two() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        let i36 = m.custom_width_int_type(36).expect("i36");
        assert_eq!(dl.type_store_size(i36.as_type()), 5);
        assert_eq!(dl.type_store_size(m.x86_fp80_type().as_type()), 10);
    })
}

/// `llvmkit-specific`: mirrors `DataLayout::getTypeAllocSize` for the
/// `i64:32:64` int spec arm: alloc-size includes ABI alignment
/// padding.
#[test]
fn type_alloc_size_i64_default() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        // Default i64 spec is `i64:32:64` (ABI=4, pref=8). So an i64
        // alloc-size rounds 8 up to 4-byte alignment -- still 8.
        assert_eq!(dl.type_alloc_size(m.i64_type().as_type()), 8);
    })
}

/// `llvmkit-specific`: mirrors `DataLayout::getABITypeAlign` for
/// integers using the default `i32:32:32` spec.
#[test]
fn abi_type_align_i32_default() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        assert_eq!(
            dl.abi_type_align(m.i32_type().as_type()),
            Align::new(4).expect("a")
        );
        assert_eq!(
            dl.abi_type_align(m.f64_type().as_type()),
            Align::new(8).expect("a")
        );
    })
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
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        let i32_ty = m.i32_type();
        let i64_ty = m.i64_type();
        let st = m.struct_type([i32_ty.as_type(), i64_ty.as_type()], false);
        let layout = dl.struct_layout(st.as_type());
        assert_eq!(layout.element_offset(0), 0);
        // i64 default ABI alignment is 4, so i64 placed at offset 4.
        assert_eq!(layout.element_offset(1), 4);
        assert_eq!(layout.size_in_bytes(), 12);
    })
}

/// `llvmkit-specific`: mirrors `StructLayout::StructLayout` packed
/// arm: every field has alignment 1.
#[test]
fn struct_layout_packed() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        let i8_ty = m.i8_type();
        let i32_ty = m.i32_type();
        let st = m.struct_type([i8_ty.as_type(), i32_ty.as_type()], true);
        let layout = dl.struct_layout(st.as_type());
        assert_eq!(layout.element_offset(0), 0);
        assert_eq!(layout.element_offset(1), 1);
        assert_eq!(layout.size_in_bytes(), 5);
    })
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

/// `llvmkit-specific`: confirms `MaybeAlign` integration with
/// `value_or_abi_type_align`. Mirrors
/// `DataLayout::getValueOrABITypeAlignment`.
#[test]
fn value_or_abi_type_align() {
    Module::with_new("m", |m| {
        let dl = DataLayout::default();
        let custom = MaybeAlign::from(Align::new(16).expect("a"));
        assert_eq!(
            dl.value_or_abi_type_align(custom.align(), m.i32_type().as_type()),
            Align::new(16).expect("a")
        );
        assert_eq!(
            dl.value_or_abi_type_align(MaybeAlign::default().align(), m.i32_type().as_type()),
            Align::new(4).expect("a")
        );
    })
}
