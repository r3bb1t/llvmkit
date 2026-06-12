//! Minimal parser corpus integration test.
//!
//! This harness intentionally uses a checked-in fixture manifest and the
//! public parser facade entry points for focused regression coverage.

use llvmkit_asmparser::parser;
use llvmkit_ir::Module;
use std::path::Path;

const CORPUS_MANIFEST: &str = include_str!("fixtures/parser_corpus_manifest.txt");

fn fixture_entries() -> Vec<&'static str> {
    CORPUS_MANIFEST
        .lines()
        .map(str::trim)
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|line| {
            line.split_once('|')
                .map(|(path, _upstream)| path.trim())
                .unwrap_or_else(|| line)
        })
        .collect()
}

/// Mirrors `llvm/lib/AsmParser/Parser.cpp` fixture loading behavior via
/// `parseAssemblyFile` and canonical smoke-verification of each checked-in corpus
/// member.
#[test]
fn parser_corpus_round_trips_checked_in_fixtures() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    for fixture in fixture_entries() {
        let path = fixture_dir.join(fixture);
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or(fixture);
        let module = Module::new(name);

        parser::parse_assembly_file_into(&path, &module)
            .unwrap_or_else(|err| panic!("corpus fixture {fixture} should parse: {err}"));
        module
            .verify_borrowed()
            .unwrap_or_else(|err| panic!("corpus fixture {fixture} should verify: {err}"));
    }
}
