//! Drift lock: `llvmkit_ir::dwarf`'s tables against the vendored `.def` files.
//!
//! Modelled on `attribute_td_drift.rs`, and possible for the same reason: the
//! inputs are vendored under this crate's `tablegen/` and **tracked**, unlike
//! `orig_cpp/`, which is gitignored and therefore unreadable from a test that
//! has to pass in CI too.
//!
//! Each case re-derives a family straight from the `.def` text and compares it
//! to the shipped table, so an LLVM bump that adds, removes, or renumbers a
//! constant fails here rather than silently teaching the parser to accept a
//! spelling upstream rejects — or to reject one it accepts.

use llvmkit_ir::dwarf;

const DWARF_DEF: &str = include_str!("../tablegen/llvm-22.1.4/include/llvm/BinaryFormat/Dwarf.def");
const DEBUG_INFO_FLAGS_DEF: &str =
    include_str!("../tablegen/llvm-22.1.4/include/llvm/IR/DebugInfoFlags.def");

/// Every `HANDLE_<macro>(ID, NAME, ...)` row, as `(name, value)`.
///
/// Mirrors how `Dwarf.cpp` consumes the same file: the first two macro
/// arguments are always the encoding and the bare name, whatever else follows.
fn rows(source: &str, macro_name: &str) -> Vec<(String, u32)> {
    let prefix = format!("HANDLE_{macro_name}(");
    source
        .lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .filter_map(|rest| {
            let (id, rest) = rest.split_once(',')?;
            let name = rest.split(['(', ',', ')']).next()?.trim();
            let id = id.trim();
            let value = id.strip_prefix("0x").map_or_else(
                || id.parse::<u32>().ok(),
                |hex| u32::from_str_radix(hex, 16).ok(),
            )?;
            Some((name.to_owned(), value))
        })
        .collect()
}

/// Assert that `derived` (from the `.def`) and the shipped table agree on both
/// spelling and encoding, in both directions.
fn assert_family(
    derived: &[(String, u32)],
    spell: impl Fn(&str) -> String,
    lookup: impl Fn(&str) -> Option<u32>,
    reverse: impl Fn(u32) -> Option<&'static str>,
    family: &str,
) {
    assert!(
        !derived.is_empty(),
        "{family}: derived no rows from the vendored .def — the macro name or file layout moved"
    );
    for (name, value) in derived {
        let spelled = spell(name);
        assert_eq!(
            lookup(&spelled),
            Some(*value),
            "{family}: `{spelled}` should encode as {value:#x}"
        );
        // The reverse direction is what the AsmWriter uses to print an
        // encoding back as a name. Upstream's own tables alias some values
        // (`DIFlagLittleEndian` and `DIFlagLargest` share a bit), so this only
        // requires that *some* spelling maps back, not that it is this one.
        assert!(
            reverse(*value).is_some(),
            "{family}: {value:#x} has no spelling"
        );
    }
}

/// Mirrors `dwarf::getTag` / `TagString` (`lib/BinaryFormat/Dwarf.cpp`).
#[test]
fn dw_tag_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_TAG"),
        |n| format!("DW_TAG_{n}"),
        dwarf::tag,
        dwarf::tag_string,
        "DW_TAG",
    );
}

/// Mirrors `dwarf::getAttributeEncoding` / `AttributeEncodingString`.
#[test]
fn dw_ate_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_ATE"),
        |n| format!("DW_ATE_{n}"),
        dwarf::attribute_encoding,
        dwarf::attribute_encoding_string,
        "DW_ATE",
    );
}

/// Mirrors `dwarf::getLanguage` / `LanguageString`.
#[test]
fn dw_lang_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_LANG"),
        |n| format!("DW_LANG_{n}"),
        dwarf::language,
        dwarf::language_string,
        "DW_LANG",
    );
}

/// Mirrors `dwarf::getSourceLanguageName` / `SourceLanguageNameString`.
#[test]
fn dw_lname_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_LNAME"),
        |n| format!("DW_LNAME_{n}"),
        dwarf::source_language_name,
        dwarf::source_language_name_string,
        "DW_LNAME",
    );
}

/// Mirrors `dwarf::getCallingConvention` / `ConventionString`.
#[test]
fn dw_cc_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_CC"),
        |n| format!("DW_CC_{n}"),
        dwarf::calling_convention,
        dwarf::calling_convention_string,
        "DW_CC",
    );
}

/// Mirrors `dwarf::getVirtuality` / `VirtualityString`.
#[test]
fn dw_virtuality_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_VIRTUALITY"),
        |n| format!("DW_VIRTUALITY_{n}"),
        dwarf::virtuality,
        dwarf::virtuality_string,
        "DW_VIRTUALITY",
    );
}

/// Mirrors `dwarf::getOperationEncoding`, whose `HANDLE_DW_OP` half comes from
/// the `.def`. Its eight hand-listed `DW_OP_LLVM_*` cases are covered by
/// [`llvm_dwarf_operations_are_present`] instead, since they are not in the file.
#[test]
fn dw_op_table_matches_the_vendored_def() {
    assert_family(
        &rows(DWARF_DEF, "DW_OP"),
        |n| format!("DW_OP_{n}"),
        dwarf::operation_encoding,
        dwarf::operation_encoding_string,
        "DW_OP",
    );
}

/// The eight `DW_OP_LLVM_*` operations `dwarf::getOperationEncoding` lists by
/// hand after including the `.def` (`lib/BinaryFormat/Dwarf.cpp`). Their values
/// are the `DW_OP_LLVM_*` enumerators in `BinaryFormat/Dwarf.h`.
#[test]
fn llvm_dwarf_operations_are_present() {
    for (spelling, value) in [
        ("DW_OP_LLVM_fragment", 0x1000),
        ("DW_OP_LLVM_convert", 0x1001),
        ("DW_OP_LLVM_tag_offset", 0x1002),
        ("DW_OP_LLVM_entry_value", 0x1003),
        ("DW_OP_LLVM_implicit_pointer", 0x1004),
        ("DW_OP_LLVM_arg", 0x1005),
        ("DW_OP_LLVM_extract_bits_sext", 0x1006),
        ("DW_OP_LLVM_extract_bits_zext", 0x1007),
    ] {
        assert_eq!(
            dwarf::operation_encoding(spelling),
            Some(value),
            "{spelling}"
        );
        assert_eq!(dwarf::operation_encoding_string(value), Some(spelling));
    }
}

/// `HANDLE_DW_OP_LLVM_USEROP` must **not** leak into the lookup table: upstream
/// uses that family only in `LlvmUserOperationEncodingString`, for printing, so
/// accepting `DW_OP_LLVM_nop` as an operation name would be a divergence.
#[test]
fn llvm_userop_family_is_not_a_parseable_operation() {
    let userops = rows(DWARF_DEF, "DW_OP_LLVM_USEROP");
    assert!(!userops.is_empty(), "the USEROP family moved");
    for (name, _) in userops {
        let spelled = format!("DW_OP_LLVM_{name}");
        // The eight hand-listed operations above are a different set and do
        // legitimately parse; nothing else from this family may.
        if dwarf::operation_encoding(&spelled).is_some() {
            assert!(
                [
                    "DW_OP_LLVM_fragment",
                    "DW_OP_LLVM_convert",
                    "DW_OP_LLVM_tag_offset",
                    "DW_OP_LLVM_entry_value",
                    "DW_OP_LLVM_implicit_pointer",
                    "DW_OP_LLVM_arg",
                    "DW_OP_LLVM_extract_bits_sext",
                    "DW_OP_LLVM_extract_bits_zext",
                ]
                .contains(&spelled.as_str()),
                "{spelled} parses as an operation but upstream only prints it"
            );
        }
    }
}

/// `dwarf::getMacinfo` is a hand-written switch in `lib/BinaryFormat/Dwarf.cpp`
/// — there is no `HANDLE_DW_MACINFO` family — so this pins the five cases and
/// that the family is still absent from the `.def`.
#[test]
fn dw_macinfo_matches_the_hand_written_switch() {
    assert!(
        rows(DWARF_DEF, "DW_MACINFO").is_empty(),
        "Dwarf.def gained a HANDLE_DW_MACINFO family; derive the table from it instead"
    );
    for (spelling, value) in [
        ("DW_MACINFO_define", 0x01),
        ("DW_MACINFO_undef", 0x02),
        ("DW_MACINFO_start_file", 0x03),
        ("DW_MACINFO_end_file", 0x04),
        ("DW_MACINFO_vendor_ext", 0xff),
    ] {
        assert_eq!(dwarf::macinfo(spelling), Some(value), "{spelling}");
    }
}

/// Mirrors `DINode::getFlag` / `getFlagString` (`lib/IR/DebugInfoMetadata.cpp`),
/// which builds its `StringSwitch` from `HANDLE_DI_FLAG` with a `DIFlag` prefix.
#[test]
fn di_flag_table_matches_the_vendored_def() {
    let derived = flag_rows(DEBUG_INFO_FLAGS_DEF, "DI_FLAG");
    assert!(!derived.is_empty(), "HANDLE_DI_FLAG rows moved");
    for (name, value) in derived {
        let spelled = format!("DIFlag{name}");
        assert_eq!(
            dwarf::di_flag(&spelled),
            Some(value),
            "{spelled} should be {value:#x}"
        );
    }
}

/// Mirrors `DISubprogram::getFlag` / `getFlagString`, built from
/// `HANDLE_DISP_FLAG` with a `DISPFlag` prefix.
#[test]
fn disp_flag_table_matches_the_vendored_def() {
    let derived = flag_rows(DEBUG_INFO_FLAGS_DEF, "DISP_FLAG");
    assert!(!derived.is_empty(), "HANDLE_DISP_FLAG rows moved");
    for (name, value) in derived {
        let spelled = format!("DISPFlag{name}");
        assert_eq!(
            dwarf::disp_flag(&spelled),
            Some(value),
            "{spelled} should be {value:#x}"
        );
    }
}

/// `DebugInfoFlags.def` writes its values as C expressions (`1`, `(1 << 2)`,
/// `(1 << 2) | (1 << 5)`, `1u`), so they are evaluated rather than parsed.
fn flag_rows(source: &str, macro_name: &str) -> Vec<(String, u32)> {
    let prefix = format!("HANDLE_{macro_name}(");
    source
        .lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .filter_map(|rest| {
            let close = rest.rfind(')')?;
            let inner = &rest[..close];
            let (expr, name) = inner.rsplit_once(',')?;
            Some((name.trim().to_owned(), eval_flag_expr(expr)))
        })
        .collect()
}

/// Evaluate the `|`-joined, `<<`-shifted integer expressions the `.def` uses.
fn eval_flag_expr(expr: &str) -> u32 {
    expr.split('|')
        .map(|term| {
            let term = term.trim().trim_matches(['(', ')']).trim();
            match term.split_once("<<") {
                Some((lhs, rhs)) => parse_int(lhs) << parse_int(rhs),
                None => parse_int(term),
            }
        })
        .fold(0, |acc, bit| acc | bit)
}

fn parse_int(text: &str) -> u32 {
    text.trim()
        .trim_matches(['(', ')'])
        .trim()
        .trim_end_matches(['u', 'U'])
        .trim()
        .parse()
        .expect("integer literal in DebugInfoFlags.def")
}
