//! Parser corpus integration test.
//!
//! This harness drives a checked-in fixture manifest through the public parser
//! facade. The manifest is the fixture-level provenance record: every row names
//! the upstream test it was copied from, and the status column says what
//! upstream's own `RUN` line asserts about it.
//!
//! Statuses:
//!
//! - `pass` --- upstream runs `llvm-as` (or `opt`) over it successfully, so the
//!   fixture must parse, verify, and be **round-trip stable**: printing it,
//!   re-parsing the print, and printing again must reproduce the first print
//!   byte for byte. That last law has no single upstream counterpart --- it is
//!   the llvmkit-side statement of what `llvm-as < %s | llvm-dis | llvm-as |
//!   llvm-dis` asserts across the many fixtures that spell it out.
//! - `reject` --- upstream runs `not llvm-as` (or `not opt`) over it, so the
//!   parse must fail. When upstream's `FileCheck` line pins the diagnostic, the
//!   row carries it in `error=` and llvmkit's rendered message must contain it,
//!   which is exactly `FileCheck`'s own substring rule. When upstream pins
//!   `<stdin>:LINE:COL:` as well, the row carries `loc=` and the reported span
//!   must start there.
//! - `xfail-parse` / `xfail-verify` --- llvmkit gaps: a fixture upstream
//!   *accepts* that llvmkit does not yet parse or verify. These are the
//!   explicit allowlist, and each one is accounted for in
//!   `docs/fixture-coverage.md`.
//!
//! Fixtures under `fixtures/upstream/assembler-corpus/` are byte-for-byte copies of
//! `llvm/test/Assembler/*.ll`; the ones in a subdirectory are the exact
//! `split-file` output for one part of a multi-part container, which is the
//! text upstream's own `RUN` line feeds to `llvm-as`.

use llvmkit_asmparser::parser;
use llvmkit_ir::Module;
use std::fs::{read, read_to_string};
use std::path::{Path, PathBuf};

pub mod support;

use support::line_and_column;

const CORPUS_MANIFEST: &str = include_str!("fixtures/parser_corpus_manifest.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorpusStatus {
    Pass,
    Reject,
    XfailParse,
    XfailVerify,
}

#[derive(Debug)]
struct CorpusEntry<'a> {
    fixture: &'a str,
    expected: Option<&'a str>,
    error: Option<&'a str>,
    loc: Option<(u32, u32)>,
    status: CorpusStatus,
    /// `config=allow-incomplete-ir` --- upstream's `-allow-incomplete-ir`
    /// `cl::opt`, which a few fixtures' `RUN` lines pass to `opt`.
    allow_incomplete_ir: bool,
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
    let mut error = None;
    let mut loc = None;
    let mut status = CorpusStatus::Pass;
    let mut allow_incomplete_ir = false;

    for option in parts {
        if let Some(path) = option.strip_prefix("expect=") {
            expected = Some(path.trim());
        } else if let Some(text) = option.strip_prefix("error=") {
            error = Some(text.trim());
        } else if let Some(pin) = option.strip_prefix("loc=") {
            loc = Some(parse_loc(pin.trim(), line));
        } else if let Some(value) = option.strip_prefix("config=") {
            match value.trim() {
                "allow-incomplete-ir" => allow_incomplete_ir = true,
                other => panic!("unknown parser corpus config `{other}` in `{line}`"),
            }
        } else if let Some(value) = option.strip_prefix("status=") {
            status = match value.trim() {
                "pass" => CorpusStatus::Pass,
                "reject" => CorpusStatus::Reject,
                "xfail-parse" => CorpusStatus::XfailParse,
                "xfail-verify" => CorpusStatus::XfailVerify,
                other => panic!("unknown parser corpus status `{other}` in `{line}`"),
            };
        }
    }

    CorpusEntry {
        fixture,
        expected,
        error,
        loc,
        status,
        allow_incomplete_ir,
    }
}

fn parse_loc(pin: &str, manifest_row: &str) -> (u32, u32) {
    let (line, column) = pin
        .split_once(':')
        .unwrap_or_else(|| panic!("malformed `loc=` pin `{pin}` in `{manifest_row}`"));
    let parse = |field: &str| {
        field
            .parse::<u32>()
            .unwrap_or_else(|err| panic!("malformed `loc=` pin `{pin}` in `{manifest_row}`: {err}"))
    };
    (parse(line), parse(column))
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Mirrors `llvm/lib/AsmParser/Parser.cpp` fixture loading behavior via
/// `parseAssemblyFile`, over the whole checked-in corpus. Every row's upstream
/// provenance is in `crates/llvmkit-asmparser/tests/fixtures/parser_corpus_manifest.txt`
/// and the classification of the full `llvm/test/Assembler` directory --- what
/// is here, what is blocked and on which gap --- is in `docs/fixture-coverage.md`.
#[test]
fn parser_corpus_round_trips_checked_in_fixtures() {
    let fixture_dir = fixture_dir();

    for entry in fixture_entries() {
        let path = fixture_dir.join(entry.fixture);
        let source = read(&path)
            .unwrap_or_else(|err| panic!("corpus fixture {} should read: {err}", entry.fixture));

        let config = parser::ParserConfig {
            allow_incomplete_ir: entry.allow_incomplete_ir,
            ..parser::ParserConfig::DEFAULT
        };
        let parse_result =
            parser::parse_assembly_file_with_config(&path, &config, |module, _parsed| {
                let printed = format!("{module}");

                if let Some(expected) = entry.expected {
                    let expected_text =
                        read_to_string(fixture_dir.join(expected)).unwrap_or_else(|err| {
                            panic!("expected output {expected} should read: {err}")
                        });
                    let expected_text = expected_text.replace("\r\n", "\n");
                    assert_eq!(
                        printed, expected_text,
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
                    CorpusStatus::Reject | CorpusStatus::XfailParse => {}
                }

                printed
            });

        match entry.status {
            CorpusStatus::Reject => {
                let error = match parse_result {
                    Ok(_) => panic!(
                        "corpus fixture {} parsed, but upstream's RUN line guards it with `not`",
                        entry.fixture
                    ),
                    Err(error) => error,
                };
                let rendered = format!("{error}");
                if let Some(pin) = entry.error {
                    assert!(
                        rendered.contains(pin),
                        "corpus fixture {} should report upstream's diagnostic `{pin}`, reported `{rendered}`",
                        entry.fixture
                    );
                }
                if let Some((line, column)) = entry.loc {
                    let start = error
                        .loc()
                        .unwrap_or_else(|| {
                            panic!(
                                "corpus fixture {} pins a location but reported none",
                                entry.fixture
                            )
                        })
                        .span
                        .start;
                    let offset = usize::try_from(start).unwrap_or(usize::MAX);
                    assert_eq!(
                        line_and_column(&source, offset),
                        (line, column),
                        "corpus fixture {} should report upstream's diagnostic location",
                        entry.fixture
                    );
                }
            }
            CorpusStatus::XfailParse => {
                if parse_result.is_ok() {
                    panic!("corpus fixture {} unexpectedly parsed", entry.fixture);
                }
            }
            CorpusStatus::XfailVerify => {
                parse_result.unwrap_or_else(|err| {
                    panic!("corpus fixture {} should parse: {err}", entry.fixture)
                });
            }
            CorpusStatus::Pass => {
                let printed = parse_result.unwrap_or_else(|err| {
                    panic!("corpus fixture {} should parse: {err}", entry.fixture)
                });
                // `parse_assembly_file` names the module after the file, so the
                // second pass has to be handed the same name or the `ModuleID`
                // comment alone would differ.
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("asm");
                let reparsed = parser::parse_into_with_config(
                    Module::dynamic(name),
                    printed.as_bytes(),
                    &config,
                )
                .unwrap_or_else(|err| {
                    panic!(
                        "corpus fixture {}: printed module should re-parse: {err}",
                        entry.fixture
                    )
                });
                let reprinted = format!("{reparsed}");
                assert_eq!(
                    reprinted, printed,
                    "corpus fixture {} should print the same text on the second pass",
                    entry.fixture
                );
            }
        }
    }
}
