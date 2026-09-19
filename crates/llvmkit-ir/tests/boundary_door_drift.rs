//! Anti-drift guard for `BlockId`'s exception door.
//!
//! A caller's block id reaches its arena slot through the checked door,
//! `BlockId::slot_in(owner)`, everywhere but at the counted boundary sites: an
//! infallible entry that cannot refuse it yet (the F1 marker, refused by Task
//! 26) or a read-only analysis combining two sources (the F2 marker, Task 27).
//! Those read through `BlockId::slot_unchecked_at_marked_boundary`,
//! which checks nothing and trusts the marker. Nothing in the compiler ties the
//! call to the marker, so a new call without one would be an unchecked,
//! uncounted read of a caller's id that the F1 / F2 tallies never see. This
//! test is that tie: every call must sit on a line whose comment block
//! directly above it names a marker.
//!
//! No upstream counterpart: LLVM's `BasicBlock *` carries no module tag and has
//! no door to keep honest.

use std::path::{Path, PathBuf};

/// The exception door's call spelling.
const DOOR_CALL: &str = ".slot_unchecked_at_marked_boundary()";

/// The two boundary markers a call may sit under. Each is spelled in two
/// halves so that this file is not itself counted by the site tally,
/// `rg -c "boundary \(F1\)" crates/` (and the same for F2), which the task
/// reports count marker sites with.
const MARKERS: [&str; 2] = [
    concat!("// boundary ", "(F1): refused by Task 26"),
    concat!("// boundary ", "(F2): Task 27"),
];

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
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
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// The 1-based line numbers of the door calls in `source` that are **not**
/// under a marker, and the number of calls seen.
///
/// A call is a line whose code (not a `//` comment) contains [`DOOR_CALL`].
/// It is under a marker when the run of `//` comment lines directly above it
/// contains one of [`MARKERS`]; a blank line or a code line ends the run.
fn unmarked_calls(source: &str) -> (Vec<usize>, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut unmarked = Vec::new();
    let mut calls = 0;
    for (index, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || !code.contains(DOOR_CALL) {
            continue;
        }
        calls += 1;
        let marked = lines[..index]
            .iter()
            .rev()
            .map(|above| above.trim_start())
            .take_while(|above| above.starts_with("//"))
            .any(|above| MARKERS.iter().any(|marker| above.starts_with(marker)));
        if !marked {
            unmarked.push(index + 1);
        }
    }
    (unmarked, calls)
}

/// Every call of `BlockId::slot_unchecked_at_marked_boundary` in
/// `crates/llvmkit-ir/src` sits directly under an F1 or F2 boundary marker.
/// Positive control: the scanner flags an unmarked call and a call separated
/// from its marker by code, and accepts a marked one, in synthetic sources;
/// and the tree has at least one call to check, so a rename cannot make the
/// test pass vacuously.
///
/// No upstream counterpart: llvmkit-specific regression for the id currency's
/// exception door (Task 24 fix round 2).
#[test]
fn every_block_id_exception_door_call_sits_under_a_boundary_marker() {
    // Positive control: the scanner itself.
    let [f1_marker, f2_marker] = MARKERS;
    let unmarked = "fn f() {\n    // a comment, but no marker\n    let s = id.slot_unchecked_at_marked_boundary();\n}\n";
    assert_eq!(unmarked_calls(unmarked), (vec![3], 1));
    let marked = format!(
        "fn f() {{\n    {f2_marker}\n    // why the read is unchecked\n    let s = id.slot_unchecked_at_marked_boundary();\n}}\n"
    );
    assert_eq!(unmarked_calls(&marked), (Vec::new(), 1));
    let separated = format!(
        "    {f1_marker}\n    let a = 1;\n    let s = id.slot_unchecked_at_marked_boundary();\n"
    );
    assert_eq!(unmarked_calls(&separated), (vec![3], 1));

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut total = 0;
    let mut violations = Vec::new();
    for file in rust_files(&src) {
        let source = std::fs::read_to_string(&file).expect("source file reads");
        let (unmarked, calls) = unmarked_calls(&source);
        total += calls;
        for line in unmarked {
            violations.push(format!("{}:{line}", file.display()));
        }
    }
    assert!(
        total > 0,
        "no call of the exception door found under {}: was it renamed?",
        src.display()
    );
    assert!(
        violations.is_empty(),
        "exception-door calls not directly under a boundary marker:\n{}",
        violations.join("\n")
    );
}
