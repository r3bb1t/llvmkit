//! Module summary index (`^N`) parsing and printing.
//!
//! Every fixture here is vendored verbatim from `llvm/test/Assembler`. Their
//! `RUN` lines drive `llvm-as | llvm-dis`, so the `; CHECK` block of each file
//! *is* the printed index, and this file reads those lines out of the fixture
//! rather than restating them.

use llvmkit_asmparser::{ModuleSummaryIndex, parse_assembly_with_index, parse_dynamic};

/// The `^`-prefixed lines of a printed index, which is the part `llvm-dis`
/// output and a fixture's `CHECK` block have in common.
fn summary_lines(index: &ModuleSummaryIndex) -> Vec<String> {
    index
        .to_string()
        .lines()
        .filter(|line| line.starts_with('^'))
        .map(str::to_owned)
        .collect()
}

/// The expectations a fixture states through the given `FileCheck` prefixes,
/// in order.
fn check_lines(fixture: &str, prefixes: &[&str]) -> Vec<String> {
    fixture
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            prefixes
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(|rest| rest.trim().to_owned())
        })
        .collect()
}

/// Parse a fixture that carries both a module and summary entries.
fn parse_index(source: &str) -> ModuleSummaryIndex {
    parse_assembly_with_index(source, |_, parsed| {
        parsed.summary_index.expect("an index was requested")
    })
    .expect("fixture parses")
}

const THINLTO_SUMMARY: &str = include_str!("fixtures/upstream/summary/thinlto-summary.ll");
const THINLTO_SUMMARY_VISIBILITY: &str =
    include_str!("fixtures/upstream/summary/thinlto-summary-visibility.ll");
const THINLTO_MULTIPLE_SUMMARIES: &str =
    include_str!("fixtures/upstream/summary/thinlto-multiple-summaries-for-guid.ll");
const THINLTO_VTABLE_SUMMARY: &str =
    include_str!("fixtures/upstream/summary/thinlto-vtable-summary.ll");
const THINLTO_MEMPROF_SUMMARY: &str =
    include_str!("fixtures/upstream/summary/thinlto-memprof-summary.ll");
const INDEX_VALUE_ORDER: &str = include_str!("fixtures/upstream/summary/index-value-order.ll");
const SUMMARY_FLAGS: &str = include_str!("fixtures/upstream/summary/summary-flags.ll");
const SUMMARY_FLAGS2: &str = include_str!("fixtures/upstream/summary/summary-flags2.ll");
const SUMMARY_PARSING_ERROR: &str =
    include_str!("fixtures/upstream/summary/summary-parsing-error.ll");
const ASM_PATH_WRITER: &str = include_str!("fixtures/upstream/summary/asm-path-writer.ll");
const MULTI_SUMMARY_DISASSEMBLE: &str =
    include_str!("fixtures/upstream/summary/multi-summary-disassemble.ll");
const THINLTO_BLOCKCOUNT: &str =
    include_str!("fixtures/upstream/summary/thinlto-blockcount-summary.ll");
const THINLTO_FLAGS: &str = include_str!("fixtures/upstream/summary/thinlto-flags-summary.ll");
const BAD_SUMMARY1: &str = include_str!("fixtures/upstream/summary/thinlto-bad-summary1.ll");
const BAD_SUMMARY2: &str = include_str!("fixtures/upstream/summary/thinlto-bad-summary2.ll");
const BAD_SUMMARY3: &str = include_str!("fixtures/upstream/summary/thinlto-bad-summary3.ll");

/// Ported from `test/Assembler/thinlto-summary.ll`, which exercises every
/// summary type and field combination and pins the whole printed index.
///
/// One line is adjusted, and only one: upstream's `^3` check omits
/// `relbf: 256` because its `RUN` line goes through bitcode, and
/// `BitcodeWriter` records a relative block frequency only for a *per-module*
/// summary — `llvm-as` builds a combined one. `AssemblyWriter::printFunctionSummary`
/// prints it whenever the hotness is unknown and the frequency is non-zero,
/// which is what a `.ll` to `.ll` path reaches. The loss is the bitcode
/// writer's, not the printer's, so restoring it is what keeps this a port of
/// the *printer's* behaviour rather than of a round trip llvmkit has no
/// equivalent of.
#[test]
fn thinlto_summary_prints_every_field() {
    let index = parse_index(THINLTO_SUMMARY);
    let expected: Vec<String> = check_lines(THINLTO_SUMMARY, &["; CHECK:"])
        .into_iter()
        .map(|line| {
            line.replace(
                "(callee: ^15, tail: 1)",
                "(callee: ^15, relbf: 256, tail: 1)",
            )
        })
        .collect();
    assert_eq!(summary_lines(&index), expected);
}

/// Ported from `test/Assembler/thinlto-summary-visibility.ll`: `visibility:` is
/// optional on input and always printed, and the fields a summary omits take
/// their defaults.
#[test]
fn thinlto_summary_visibility_round_trips() {
    let index = parse_index(THINLTO_SUMMARY_VISIBILITY);
    let expected = check_lines(THINLTO_SUMMARY_VISIBILITY, &["; CHECK:", "; CHECK-NEXT:"]);
    // The fixture checks only the three `gv` entries; the module entries and
    // the trailing block count are printed either way.
    let printed = summary_lines(&index);
    assert_eq!(
        printed
            .iter()
            .filter(|line| line.contains("= gv:"))
            .cloned()
            .collect::<Vec<_>>(),
        expected
    );
}

/// Ported from `test/Assembler/thinlto-multiple-summaries-for-guid.ll`: one
/// GUID carrying two function summaries, and the `[Regular LTO]` module path.
#[test]
fn thinlto_multiple_summaries_for_one_guid_round_trip() {
    let index = parse_index(THINLTO_MULTIPLE_SUMMARIES);
    let expected = check_lines(THINLTO_MULTIPLE_SUMMARIES, &["; CHECK:", "; CHECK-NEXT:"]);
    let printed = summary_lines(&index);
    assert_eq!(printed[..expected.len()], expected[..]);
}

/// Ported from `test/Assembler/thinlto-vtable-summary.ll`, whose `RUN` lines
/// `diff` the `^` lines of the input against the `^` lines of the output — so
/// the fixture's own summary block is the expected text, with no bitcode round
/// trip in between. It is the one fixture here that reaches the `name:` form,
/// so it also pins GUID computation against a real module.
#[test]
fn thinlto_vtable_summary_round_trips() {
    let index = parse_index(THINLTO_VTABLE_SUMMARY);
    let expected: Vec<String> = THINLTO_VTABLE_SUMMARY
        .lines()
        .filter(|line| line.starts_with('^'))
        .map(str::to_owned)
        .collect();
    assert_eq!(summary_lines(&index), expected);
}

/// Ported from `test/Assembler/thinlto-memprof-summary.ll`, the `CONTEXT`
/// half. The `NOCONTEXT` half has no llvmkit counterpart: it is produced by
/// `llvm-as -combined-index-memprof-context=false`, a bitcode-writer option
/// that drops the allocation context stack ids, and llvmkit has no bitcode.
///
/// The fixture's checks stop at the last `gv` entry; the trailing block count
/// `printModuleSummaryIndex` always emits is simply not among them.
#[test]
fn thinlto_memprof_summary_round_trips() {
    let index = parse_index(THINLTO_MEMPROF_SUMMARY);
    let expected = check_lines(THINLTO_MEMPROF_SUMMARY, &["; CHECK:", "; CONTEXT:"]);
    let printed = summary_lines(&index);
    assert_eq!(printed[..expected.len()], expected[..]);
    assert_eq!(printed[expected.len()], "^6 = blockcount: 0");
}

/// Ported from `test/Assembler/index-value-order.ll`: summary ids that arrive
/// out of order still parse, and the printer renumbers them. The fixture
/// states its expectations as `CHECK-DAG` lines bound to `^[[VTBL]]` and
/// `^[[VFN]]`, which is what the bindings below reproduce.
#[test]
fn index_value_order_renumbers_summary_ids() {
    let index = parse_index(INDEX_VALUE_ORDER);
    let printed = summary_lines(&index);

    let slot_of = |name: &str| -> String {
        let line = printed
            .iter()
            .find(|line| line.contains(&format!("\"{name}\"")) && line.contains("= gv:"))
            .unwrap_or_else(|| panic!("a gv entry for {name}"));
        line.split(" = ")
            .next()
            .expect("every entry starts with its slot")
            .to_owned()
    };

    let vtable = slot_of("_ZTVN3FooE");
    let virtual_function = slot_of("_Z3barv");
    assert!(
        printed
            .iter()
            .any(|line| line.starts_with(&format!("{vtable} ="))
                && line.contains(&format!("virtFunc: {virtual_function}")))
    );
    assert!(printed.iter().any(
        |line| line.contains("typeidCompatibleVTable: (name: \"_ZTSN3FooE\"")
            && line.contains(&format!("(offset: 16, {vtable})"))
    ));
    assert!(
        printed
            .iter()
            .any(|line| line.contains("= gv: (name: \"_ZTSN3FooE\""))
    );
}

/// Ported from `test/Assembler/summary-flags.ll`: the index flags survive a
/// round trip, and are numbered after every other entry.
#[test]
fn summary_flags_round_trip() {
    let index = parse_index(SUMMARY_FLAGS);
    let printed = summary_lines(&index);
    assert!(printed[0].starts_with("^0 = module"));
    assert!(printed[1].starts_with("^1 = gv"));
    assert_eq!(printed[2], "^2 = flags: 97");
    assert_eq!(index.flags().raw(), 97);
}

/// Ported from `test/Assembler/summary-flags2.ll`: an otherwise empty index
/// with non-trivial flags.
#[test]
fn summary_flags_alone_round_trip() {
    let index = parse_index(SUMMARY_FLAGS2);
    assert_eq!(
        summary_lines(&index),
        vec!["^0 = flags: 2".to_owned(), "^1 = blockcount: 0".to_owned()]
    );
}

/// Ported from `test/Assembler/asm-path-writer.ll`: a module path containing
/// backslashes is escaped on the way out.
#[test]
fn module_path_is_escaped() {
    let index = parse_index(ASM_PATH_WRITER);
    let expected = check_lines(ASM_PATH_WRITER, &["; CHECK:"]);
    assert_eq!(summary_lines(&index)[0], expected[0]);
}

/// Ported from `test/Assembler/multi-summary-disassemble.ll`, whose `CHECK`
/// lines pin the module entry and the leading half of the `gv` entry.
#[test]
fn multi_summary_disassemble_prints_its_entries() {
    let index = parse_index(MULTI_SUMMARY_DISASSEMBLE);
    let printed = summary_lines(&index);
    for expected in check_lines(MULTI_SUMMARY_DISASSEMBLE, &["; CHECK:"]) {
        assert!(
            printed.iter().any(|line| line.starts_with(&expected)),
            "no printed line starts with {expected:?}"
        );
    }
}

/// Ported from `test/Assembler/thinlto-blockcount-summary.ll`, whose `RUN`
/// line only requires that the file parse.
#[test]
fn blockcount_summary_parses() {
    let index = parse_index(THINLTO_BLOCKCOUNT);
    assert_eq!(index.block_count(), 1234);
}

/// Ported from `test/Assembler/thinlto-flags-summary.ll`, whose `RUN` line only
/// requires that the file parse.
#[test]
fn flags_summary_parses() {
    let index = parse_index(THINLTO_FLAGS);
    assert_eq!(index.flags().raw(), 8);
}

/// Ported from `test/Assembler/summary-parsing-error.ll`: a `name:` entry whose
/// name is not in the module. Reachable only with a module in hand, which is
/// the mode `llvm-as` parses in.
#[test]
fn summary_name_must_exist_in_the_module() {
    let error = parse_assembly_with_index(SUMMARY_PARSING_ERROR, |_, _| ())
        .expect_err("the name is undefined");
    assert_eq!(
        error.to_string(),
        "Reference to undefined global \"does_not_exist\""
    );
}

/// Ported from `test/Assembler/thinlto-bad-summary1.ll`. Its `RUN` line is
/// `opt`, which parses with no summary index, so the entry is skipped rather
/// than parsed — and the skip has a keyword guard of its own.
#[test]
fn summary_entry_without_an_index_needs_a_known_tag() {
    let error = parse_dynamic(BAD_SUMMARY1).expect_err("`^1 = ()` has no tag");
    assert_eq!(
        error.to_string(),
        "Expected 'gv', 'module', 'typeid', 'flags' or 'blockcount' at the start of summary entry"
    );
}

/// Ported from `test/Assembler/thinlto-bad-summary2.ll`.
#[test]
fn skipped_summary_entry_needs_its_open_paren() {
    let error = parse_dynamic(BAD_SUMMARY2).expect_err("`^1 = gv: )(` is malformed");
    assert_eq!(error.to_string(), "expected '(' at start of summary entry");
}

/// Ported from `test/Assembler/thinlto-bad-summary3.ll`.
#[test]
fn skipped_summary_entry_reports_unbalanced_parentheses() {
    let error = parse_dynamic(BAD_SUMMARY3).expect_err("a ')' is missing");
    assert_eq!(
        error.to_string(),
        "found end of file while parsing summary entry"
    );
}

/// llvmkit-specific (no upstream counterpart): `LLParser::parseFlag` takes the
/// token's **boolean** value, so any non-zero unsigned integer reads as set,
/// and rejects a *signed* token — which `s0x1` is and `u0x1` is not, whatever
/// they look like. `test/Assembler` spells every flag as `0` or `1`, so the
/// routine is the anchor.
#[test]
fn summary_flags_take_the_tokens_boolean_value() {
    let with_live = |value: &str| {
        format!(
            "^0 = module: (path: \"m.o\", hash: (0, 0, 0, 0, 0))\n\
             ^1 = gv: (guid: 1, summaries: (function: (module: ^0, \
             flags: (linkage: external, live: {value}), insts: 1)))\n"
        )
    };

    for (spelling, expected) in [("0", false), ("1", true), ("5", true), ("u0x2", true)] {
        let index = parse_index(&with_live(spelling));
        let summary = &index
            .global_values()
            .values()
            .next()
            .expect("one global value")
            .summary_list[0];
        assert_eq!(summary.flags.live, expected, "live: {spelling}");
    }

    for spelling in ["s0x1", "-1"] {
        let error = parse_assembly_with_index(with_live(spelling), |_, _| ())
            .expect_err("a signed flag token is rejected");
        assert_eq!(error.to_string(), "expected integer", "live: {spelling}");
    }
}

/// llvmkit-specific (no upstream counterpart): `parse_dynamic` is the entry
/// point that mirrors `parseAssembly` with a null index, so a well-formed
/// summary entry is skipped whole and no index is produced.
#[test]
fn parsing_without_an_index_skips_summary_entries() {
    let parsed = llvmkit_asmparser::parse_assembly(THINLTO_MULTIPLE_SUMMARIES, |_, parsed| {
        parsed.summary_index.is_none()
    })
    .expect("the summary entries are skipped");
    assert!(parsed);
}

/// Ported from `test/Assembler/thinlto-summary.ll` read through
/// `parseSummaryIndexAssembly`: with no module, every non-summary entity is
/// lexed past and the index still comes out whole.
#[test]
fn index_only_mode_reads_past_module_entities() {
    let index = llvmkit_asmparser::parse_summary_index_assembly(THINLTO_VTABLE_SUMMARY)
        .expect("the index parses on its own");
    // The `name:` entries compute their GUIDs from the source file name rather
    // than from a module, and `thinlto-vtable-summary.ll` gives one.
    assert_eq!(index.module_paths().len(), 1);
    assert_eq!(index.global_values().len(), 5);
    assert_eq!(index.type_id_compatible_vtables().len(), 3);
}
