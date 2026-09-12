//! Anti-drift guard for `UPSTREAM.md`'s row targets.
//!
//! Every row in the provenance registry opens with a backticked
//! `path/to/file.rs::test_name` (or a bare `path/to/file.rs` for a row that
//! covers a whole file or a trybuild fixture). Nothing tied those to the tree,
//! and they rotted exactly the way a hand-maintained table does: a test moved
//! file and its row did not follow. Eleven rows had been pointing at
//! `crates/llvmkit-ir/tests/{builder_typestate_termination,constant_folding_analysis,verifier_basic}.rs`
//! for tests that live in `crates/llvmkit-ir/src/phi_raw_tests/`, and a
//! narrower earlier sweep missed them because the rows sit next to siblings
//! that still resolve.
//!
//! Two tests here assert what the registry claims: the cited file exists, and
//! when the row names a test, that file defines it. A third walks the other
//! way — every `#[test]` in the tree must carry a row or a line in the frozen
//! `tests/fixtures/upstream_provenance_debt.txt` — so the provenance backlog
//! can shrink but not grow.
//!
//! None of them says anything about whether the *upstream* citation in the
//! second column is right; that is a review judgement, not a mechanical one.
//!
//! No upstream counterpart: LLVM has no provenance registry to keep honest.

use std::path::{Path, PathBuf};

const REGISTRY: &str = include_str!("../../../UPSTREAM.md");

/// The frozen backlog of tests that carry no registry row — see
/// [`every_test_carries_a_registry_row_or_a_line_in_the_frozen_debt_list`].
const PROVENANCE_DEBT: &str = include_str!("fixtures/upstream_provenance_debt.txt");

/// The repository root, reached from this crate's manifest directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repository root resolves from CARGO_MANIFEST_DIR")
}

/// The first backticked cell of every table row, as `(path, test name)`.
///
/// A row is a line opening `` | ` ``; its first cell runs to the closing
/// backtick. Anything after that on the line (a `(whole file)` rider, a second
/// backticked name in a range row) is the row's prose and is not a target.
fn row_targets() -> Vec<(&'static str, Option<&'static str>)> {
    REGISTRY
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|rest| rest.split('`').next())
        .filter(|cell| !cell.is_empty())
        // The path runs to the *first* `::`; the test name follows the
        // *last*, so a row naming an in-file `mod tests` resolves too.
        .map(
            |cell| match (cell.split_once("::"), cell.rsplit_once("::")) {
                (Some((path, _)), Some((_, name))) => (path, Some(name)),
                _ => (cell, None),
            },
        )
        .collect()
}

/// Every `UPSTREAM.md` row names a file that exists.
#[test]
fn every_registry_row_names_a_file_in_the_tree() {
    let root = repo_root();
    let mut missing = Vec::new();
    for (path, _) in row_targets() {
        if !root.join(path).is_file() {
            missing.push(path);
        }
    }
    missing.sort_unstable();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "UPSTREAM.md rows name files that do not exist:\n{}",
        missing.join("\n")
    );
}

/// Every `UPSTREAM.md` row that names a test names one the cited file defines.
///
/// The match is on the `fn <name>` token rather than on a parse of the file:
/// the registry's contract is that a reader can open the cited file and find
/// the test, and that is exactly what this checks.
#[test]
fn every_registry_row_names_a_test_its_cited_file_defines() {
    let root = repo_root();
    let mut unresolved = Vec::new();
    for (path, name) in row_targets() {
        let Some(name) = name else { continue };
        let Ok(source) = std::fs::read_to_string(root.join(path)) else {
            // The file's own absence is the sibling test's finding.
            continue;
        };
        let defined = source
            .match_indices(&format!("fn {name}"))
            .any(|(at, matched)| {
                source[at + matched.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            });
        if !defined {
            unresolved.push(format!("{path}::{name}"));
        }
    }
    unresolved.sort_unstable();
    unresolved.dedup();
    assert!(
        unresolved.is_empty(),
        "UPSTREAM.md rows name tests their cited file does not define:\n{}",
        unresolved.join("\n")
    );
}

/// Every `#[test]` in the workspace, as `(path relative to the repo root, name)`.
///
/// The scan is deliberately literal: a line that trims to exactly `#[test]`,
/// then the first `fn <name>` at or after it. Doc comments in this tree quote
/// `` `#[test]` `` in prose often enough that a looser match invents tests —
/// `crates/llvmkit-ir/tests/analysis_basic.rs` has a `///` line saying "not a
/// `#[test]` body" directly above a helper `fn`.
fn workspace_tests(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    files.sort_unstable();

    let mut tests = Vec::new();
    for file in files {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(relative) = file.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let lines: Vec<&str> = source.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            if line.trim() != "#[test]" {
                continue;
            }
            let name = lines[at..].iter().find_map(|following| {
                let rest = following.split_once("fn ")?.1;
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            });
            if let Some(name) = name {
                tests.push((relative.clone(), name));
            }
        }
    }
    tests
}

/// **No upstream counterpart** — D11 house law, made mechanical in the one
/// direction the sibling tests do not cover.
///
/// `every_registry_row_names_a_file_in_the_tree` and its twin walk from
/// `UPSTREAM.md` to the tree. Nothing walked the other way, so a test could
/// land with no row at all — which is how the residue
/// `docs/divergences.md` calls the provenance debt accumulated in the first
/// place, silently, one commit at a time.
///
/// This test closes the *growth*, not the backlog: every `#[test]` must either
/// be covered by a row (its own, or a whole-file row for the file it lives in)
/// or appear verbatim in `tests/fixtures/upstream_provenance_debt.txt`, which
/// is frozen. A new test with no row fails here. Paying a debt line down means
/// deleting it, and a line that no longer names an uncovered test fails too, so
/// the file cannot rot in either direction.
///
/// A debt line means missing *provenance*, never "no upstream counterpart":
/// clearing one means naming a real source or saying in the row that the test
/// is llvmkit-specific.
#[test]
fn every_test_carries_a_registry_row_or_a_line_in_the_frozen_debt_list() {
    let root = repo_root();

    let mut whole_file_rows: Vec<&str> = Vec::new();
    let mut named_rows: Vec<(&str, &str)> = Vec::new();
    for (path, name) in row_targets() {
        match name {
            Some(name) => named_rows.push((path, name)),
            None => whole_file_rows.push(path),
        }
    }

    let mut uncovered: Vec<String> = workspace_tests(&root)
        .into_iter()
        .filter(|(path, name)| {
            !whole_file_rows.contains(&path.as_str())
                && !named_rows.contains(&(path.as_str(), name.as_str()))
        })
        .map(|(path, name)| format!("{path}::{name}"))
        .collect();
    uncovered.sort_unstable();
    uncovered.dedup();

    let debt: Vec<&str> = PROVENANCE_DEBT
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    let unlisted: Vec<&String> = uncovered
        .iter()
        .filter(|entry| !debt.contains(&entry.as_str()))
        .collect();
    assert!(
        unlisted.is_empty(),
        "these tests have no `UPSTREAM.md` row (D11 wants one in the same commit):\n{}",
        unlisted
            .iter()
            .map(|entry| entry.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );

    let paid: Vec<&&str> = debt
        .iter()
        .filter(|entry| !uncovered.iter().any(|found| found == **entry))
        .collect();
    assert!(
        paid.is_empty(),
        "these debt lines no longer name an unrowed test; delete them from \
         tests/fixtures/upstream_provenance_debt.txt:\n{}",
        paid.iter()
            .map(|entry| **entry)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
