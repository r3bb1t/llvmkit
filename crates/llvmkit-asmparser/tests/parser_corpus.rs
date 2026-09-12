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
//!   *accepts* that llvmkit does not yet parse or verify. Neither has a row
//!   today; a fixture llvmkit cannot handle is classified `blocked-model` in
//!   `docs/fixture-coverage.md` and gets no manifest row at all. The three
//!   rows that used to carry `xfail-parse` were upstream *negatives* misfiled
//!   as llvmkit gaps, and are `reject` rows now.
//!
//! Fixtures under `fixtures/upstream/assembler-corpus/` are byte-for-byte copies of
//! `llvm/test/Assembler/*.ll`; the ones in a subdirectory are the exact
//! `split-file` output for one part of a multi-part container, which is the
//! text upstream's own `RUN` line feeds to `llvm-as`. That last sentence used
//! to be an unchecked claim; `split_file_parts_are_what_split_file_emits`
//! below now checks it against the vendored containers in
//! `fixtures/upstream/assembler-corpus/split-file-containers/`.

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
    /// The row's second cell: the upstream test this fixture was copied from,
    /// either `test/…/foo.ll` or `test/…/foo.ll split-file part bar.ll`.
    upstream: &'a str,
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
    let upstream = parts.next().unwrap_or("");
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
        upstream,
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
        let module_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<string>");
        let parse_result =
            parser::parse_assembly_with_name(module_name, &source, &config, |module, _parsed| {
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
                    let start = error.loc().start;
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
                // Both passes are handed the same module name explicitly; the
                // `ModuleID` comment alone would differ otherwise.
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<string>");
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

/// The part `name` of a `split-file` container, rebuilt from `container`.
///
/// Mirrors `handle` in `llvm/utils/split-file/split-file.cpp`: a separator line
/// is `^(.|//)--- ` — `markerLen` is 6 when the line opens `//` and 5 otherwise,
/// and the four characters ending at `markerLen` must be `"--- "` — the part
/// runs from the line after its separator to the line before the next one, and
/// `--leading-lines` prepends `i.line_number() - 1` blank lines, where `i`
/// already sits on the part's first line. That is the separator's own 1-based
/// line number, so the part reproduces the container's line numbering exactly.
///
/// `EOL` is normalized to `\n` by the caller, so the padding is `\n`; upstream
/// pads with the container's detected `EOL`. Upstream's three `error()` arms
/// (empty part name, a name with surrounding space, a duplicate name) are not
/// mirrored: they abort the tool rather than shape a part, and the callers here
/// are the vendored containers, which have none.
fn split_file_part(container: &str, name: &str, leading_lines: bool) -> Option<String> {
    fn separator_part_name(line: &str) -> Option<&str> {
        let marker_len = if line.starts_with("//") { 6 } else { 5 };
        if line.len() >= marker_len
            && line
                .get(marker_len - 4..)
                .is_some_and(|rest| rest.starts_with("--- "))
        {
            line.get(marker_len..)
        } else {
            None
        }
    }

    let lines: Vec<&str> = container.split_inclusive('\n').collect();
    let separator = lines
        .iter()
        .position(|line| separator_part_name(line.trim_end_matches('\n')) == Some(name))?;
    let end = lines
        .iter()
        .skip(separator + 1)
        .position(|line| separator_part_name(line.trim_end_matches('\n')).is_some())
        .map_or(lines.len(), |offset| separator + 1 + offset);

    let padding = if leading_lines { separator + 1 } else { 0 };
    let mut part = "\n".repeat(padding);
    part.extend(lines[separator + 1..end].iter().copied());
    Some(part)
}

/// **No upstream counterpart** — a guard on this corpus, not on LLVM.
///
/// Every manifest row whose second cell reads `… split-file part <name>` must
/// hold exactly what `split-file` writes for that part of the vendored
/// container. Nothing checked this, and all thirty parts of the five containers
/// whose `RUN` line passes `--leading-lines` were one line short: they carried
/// `separator` blank lines where `split-file` writes `separator + 1`, so the
/// part's line numbers were one *below* the container's.
///
/// That is exactly the fixture whose numbering a `CHECK` line adjudicates.
/// `test/Assembler/ptrtoaddr-invalid-constexpr.ll` writes
/// `; SRC_NOT_PTR: [[#@LINE-1]]:17: error: …` against container line 28, and
/// the shifted part put that IR on line 27 — so a `loc=` pin taken from
/// upstream could never have matched, and one derived from the part would have
/// blessed llvmkit's own answer. The rule is pinned by `split-file`'s own test,
/// `llvm/test/tools/split-file/basic.test`: `;--- bb` on line 3 yields
/// `Inputs/basic-bb.txt`, whose first content line is line 4.
#[test]
fn split_file_parts_are_what_split_file_emits() {
    let fixture_dir = fixture_dir();
    let container_dir = fixture_dir.join("upstream/assembler-corpus/split-file-containers");
    let mut checked = 0usize;

    for entry in fixture_entries() {
        let Some((container_path, part_name)) = entry.upstream.split_once(" split-file part ")
        else {
            continue;
        };
        let container_file = Path::new(container_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| panic!("row `{}` names no container file", entry.fixture));
        let container = read_to_string(container_dir.join(container_file))
            .unwrap_or_else(|err| {
                panic!("vendored split-file container {container_file} should read: {err}")
            })
            .replace("\r\n", "\n");

        // Upstream's own `RUN` line decides whether the parts preserve line
        // numbers, so read it from the container rather than from a table here.
        let leading_lines = container.contains("--leading-lines");
        let rebuilt = split_file_part(&container, part_name.trim(), leading_lines)
            .unwrap_or_else(|| panic!("{container_file} has no `--- {part_name}` separator"));

        let actual = read_to_string(fixture_dir.join(entry.fixture))
            .unwrap_or_else(|err| panic!("corpus fixture {} should read: {err}", entry.fixture))
            .replace("\r\n", "\n");

        assert_eq!(
            actual,
            rebuilt,
            "corpus fixture {} is not `split-file{}` part `{part_name}` of {container_file}",
            entry.fixture,
            if leading_lines {
                " --leading-lines"
            } else {
                ""
            }
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "no manifest row names a split-file part; this guard has stopped guarding anything"
    );
}

/// **No upstream counterpart** — a guard on this manifest, not on LLVM.
///
/// Two rows may not name one fixture, and two fixtures may not hold identical
/// bytes. The second half is the one with teeth: `2004-11-28-InvalidTypeCrash.ll`
/// sat in the corpus twice, once at `upstream/` as `status=xfail-parse` and
/// once at `upstream/assembler-corpus/` as `status=reject | error=…`. The two
/// files were byte-identical, so the weaker row was asserting nothing the
/// stronger one did not already assert, and the contradiction between their
/// statuses went unnoticed for as long as nothing compared them.
#[test]
fn no_two_manifest_rows_name_or_hold_the_same_fixture() {
    let fixture_dir = fixture_dir();
    let mut by_path: Vec<&str> = Vec::new();
    let mut by_content: Vec<(&str, Vec<u8>)> = Vec::new();

    for entry in fixture_entries() {
        assert!(
            !by_path.contains(&entry.fixture),
            "manifest names `{}` twice",
            entry.fixture
        );
        by_path.push(entry.fixture);

        let source = read(fixture_dir.join(entry.fixture))
            .unwrap_or_else(|err| panic!("corpus fixture {} should read: {err}", entry.fixture));
        if let Some((other, _)) = by_content.iter().find(|(_, bytes)| *bytes == source) {
            panic!(
                "corpus fixtures `{}` and `{}` are byte-identical; one row is redundant",
                other, entry.fixture
            );
        }
        by_content.push((entry.fixture, source));
    }
}
