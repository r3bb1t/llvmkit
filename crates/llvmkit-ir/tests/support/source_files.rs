//! The source walker the drift tests share, included by each with
//! `#[path = "support/source_files.rs"] mod source_files;`. Integration
//! tests are separate crates, so a `#[path]` module is how they share code;
//! cargo builds only `tests/*.rs` as tests, so this file is no test of its
//! own.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively, sorted. A directory or entry
/// that cannot be read fails the calling test: a drift guard that skipped an
/// unreadable subtree would pass without checking it.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("source directory {} reads: {error}", dir.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| panic!("an entry of {} reads: {error}", dir.display()))
                .path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}
