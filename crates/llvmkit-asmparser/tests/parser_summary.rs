//! Module summary index parser tests.

use llvmkit_asmparser::module_summary::GlobalValueSummary;
use llvmkit_asmparser::parser;

const SUMMARY: &str = include_str!("fixtures/summary_minimal.ll");

fn assert_summary_round_trip(printed: &str) {
    assert_eq!(
        printed.trim_end_matches('\n'),
        SUMMARY.trim_end_matches('\n')
    );
}

/// Mirrors `LLParser.cpp::parseModuleEntry`.
#[test]
fn summary_module_entry_round_trips() {
    let index = parser::parse_summary_index_assembly(SUMMARY.as_bytes()).expect("summary parses");
    assert_eq!(index.modules.len(), 1);
    assert_eq!(index.modules[0].id, 1);
    assert_eq!(index.modules[0].path, "mod.ll");
    let printed = format!("{index}");
    assert_summary_round_trip(&printed);
}

/// Mirrors `LLParser.cpp::parseFunctionSummary`.
#[test]
fn summary_function_entry_round_trips() {
    let index = parser::parse_summary_index_assembly(SUMMARY.as_bytes()).expect("summary parses");
    let function = index
        .globals
        .iter()
        .flat_map(|gv| gv.summaries.iter())
        .find_map(|summary| match summary {
            GlobalValueSummary::Function(summary) => Some(summary),
            _ => None,
        })
        .expect("function summary present");
    assert_eq!(function.module, 1);
    assert_eq!(function.insts, 3);
    assert_eq!(function.calls.len(), 1);
    let printed = format!("{index}");
    assert_summary_round_trip(&printed);
}

/// Mirrors `LLParser.cpp::parseVariableSummary`.
#[test]
fn summary_variable_entry_round_trips() {
    let index = parser::parse_summary_index_assembly(SUMMARY.as_bytes()).expect("summary parses");
    let variable = index
        .globals
        .iter()
        .flat_map(|gv| gv.summaries.iter())
        .find_map(|summary| match summary {
            GlobalValueSummary::Variable(summary) => Some(summary),
            _ => None,
        })
        .expect("variable summary present");
    assert_eq!(variable.module, 1);
    assert_eq!(variable.refs.len(), 1);
    let printed = format!("{index}");
    assert_summary_round_trip(&printed);
}

/// Mirrors `LLParser.cpp::parseAliasSummary`.
#[test]
fn summary_alias_entry_round_trips() {
    let index = parser::parse_summary_index_assembly(SUMMARY.as_bytes()).expect("summary parses");
    let alias = index
        .globals
        .iter()
        .flat_map(|gv| gv.summaries.iter())
        .find_map(|summary| match summary {
            GlobalValueSummary::Alias(summary) => Some(summary),
            _ => None,
        })
        .expect("alias summary present");
    assert_eq!(alias.module, 1);
    assert!(alias.aliasee.is_some());
    let printed = format!("{index}");
    assert_summary_round_trip(&printed);
}

/// Mirrors `Parser.cpp::parseAssemblyFile` for summary indexes.
#[test]
fn parse_summary_index_assembly_file_reads_fixture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/summary_minimal.ll"
    );
    let index = parser::parse_summary_index_assembly_file(path).expect("summary file parses");
    assert_eq!(index.flags.as_ref().map(|f| f.raw), Some(7));
    assert_eq!(index.block_count, Some(9));
}
