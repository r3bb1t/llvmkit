//! Minimal parser corpus integration test.
//!
//! This harness intentionally uses a checked-in fixture manifest and the
//! public parser facade entry points for focused regression coverage.

use llvmkit_asmparser::parser;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

const CORPUS_MANIFEST: &str = include_str!("fixtures/parser_corpus_manifest.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorpusStatus {
    Pass,
    XfailParse,
    XfailVerify,
}

#[derive(Debug)]
struct CorpusEntry<'a> {
    fixture: &'a str,
    expected: Option<&'a str>,
    status: CorpusStatus,
}

fn fixture_entries() -> Vec<CorpusEntry<'static>> {
    CORPUS_MANIFEST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(parse_manifest_entry)
        .collect()
}

fn parse_manifest_entry(line: &'static str) -> CorpusEntry<'static> {
    let mut parts = line.split('|').map(str::trim);
    let fixture = parts.next().filter(|part| !part.is_empty()).unwrap_or(line);
    let _upstream = parts.next();
    let mut expected = None;
    let mut status = CorpusStatus::Pass;

    for option in parts {
        if let Some(path) = option.strip_prefix("expect=") {
            expected = Some(path.trim());
        } else if let Some(value) = option.strip_prefix("status=") {
            status = match value.trim() {
                "pass" => CorpusStatus::Pass,
                "xfail-parse" => CorpusStatus::XfailParse,
                "xfail-verify" => CorpusStatus::XfailVerify,
                other => panic!("unknown parser corpus status `{other}` in `{line}`"),
            };
        }
    }

    CorpusEntry {
        fixture,
        expected,
        status,
    }
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Mirrors `llvm/lib/AsmParser/Parser.cpp` fixture loading behavior via
/// `parseAssemblyFile` and canonical smoke-verification of each checked-in corpus
/// member. Manifest `status=xfail-*` entries are the explicit parser corpus
/// allowlist for upstream-negative or not-yet-supported shapes.
#[test]
fn parser_corpus_round_trips_checked_in_fixtures() {
    let fixture_dir = fixture_dir();

    for entry in fixture_entries() {
        let path = fixture_dir.join(entry.fixture);
        let parse_result = parser::parse_assembly_file(&path, |module, _parsed| {
            if let Some(expected) = entry.expected {
                let expected_text = read_to_string(fixture_dir.join(expected))
                    .unwrap_or_else(|err| panic!("expected output {expected} should read: {err}"));
                let expected_text = expected_text.replace("\r\n", "\n");
                assert_eq!(
                    format!("{module}"),
                    expected_text,
                    "corpus fixture {} should print canonically",
                    entry.fixture
                );
            }

            let verify_result = module.verify_borrowed();
            match entry.status {
                CorpusStatus::Pass => verify_result.unwrap_or_else(|err| {
                    panic!("corpus fixture {} should verify: {err}", entry.fixture)
                }),
                CorpusStatus::XfailVerify => {
                    if verify_result.is_ok() {
                        panic!("corpus fixture {} unexpectedly verified", entry.fixture);
                    }
                }
                CorpusStatus::XfailParse => {}
            }
        });
        match entry.status {
            CorpusStatus::XfailParse => {
                if parse_result.is_ok() {
                    panic!("corpus fixture {} unexpectedly parsed", entry.fixture);
                }
            }
            CorpusStatus::Pass | CorpusStatus::XfailVerify => {
                parse_result.unwrap_or_else(|err| {
                    panic!("corpus fixture {} should parse: {err}", entry.fixture)
                });
            }
        }
    }
}
