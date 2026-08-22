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
//! This test asserts what the registry claims: the cited file exists, and when
//! the row names a test, that file defines it. It says nothing about whether
//! the *upstream* citation in the second column is right — that is a review
//! judgement, not a mechanical one.
//!
//! No upstream counterpart: LLVM has no provenance registry to keep honest.

use std::path::{Path, PathBuf};

const REGISTRY: &str = include_str!("../../../UPSTREAM.md");

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
