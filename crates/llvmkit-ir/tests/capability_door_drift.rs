//! Anti-drift guard for the capability door.
//!
//! `ModuleRef::mutable_at_marked_boundary` turns a module reference of any
//! capability into a `Mutable` one and checks nothing; it is sound only where
//! the caller holds mutation authority over that very module (D8). Nothing in
//! the compiler ties a call to that authority, so every call must sit on a
//! line whose comment block directly above it names the authority with a
//! proof marker.
//!
//! No upstream counterpart: LLVM has no capability on a `Value *` and no door
//! to keep honest.

use std::path::{Path, PathBuf};

/// The capability door's call spelling.
const DOOR_CALL: &str = ".mutable_at_marked_boundary()";

/// The proof marker a call must sit under. Spelled in two halves so that this
/// file is not itself counted by `rg -c "capability \(proof\)" crates/`.
const MARKER: &str = concat!("// capability ", "(proof): ");

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
/// under a proof marker, and the number of calls seen.
///
/// A call is a line whose code (not a `//` comment) contains [`DOOR_CALL`].
/// It is under a marker when the run of `//` comment lines directly above it
/// contains [`MARKER`]; a blank line or a code line ends the run.
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
            .any(|above| above.starts_with(MARKER));
        if !marked {
            unmarked.push(index + 1);
        }
    }
    (unmarked, calls)
}

/// Every call of `ModuleRef::mutable_at_marked_boundary` in
/// `crates/llvmkit-ir/src` sits directly under a `capability (proof)` marker
/// naming the authority that makes it sound. Positive control: the scanner
/// flags an unmarked call and a call separated from its marker by code, and
/// accepts a marked one, in synthetic sources; and the tree has at least one
/// call to check, so a rename cannot make the test pass vacuously.
///
/// No upstream counterpart: llvmkit-specific guard for the capability door
/// (D8, Task 25).
#[test]
fn every_capability_door_call_names_its_authority() {
    // Positive control: the scanner itself.
    let unmarked = "fn f() {\n    // a comment, but no marker\n    let m = r.mutable_at_marked_boundary();\n}\n";
    assert_eq!(unmarked_calls(unmarked), (vec![3], 1));
    let marked = format!(
        "fn f() {{\n    {MARKER}the builder's module\n    // why it is sound\n    let m = r.mutable_at_marked_boundary();\n}}\n"
    );
    assert_eq!(unmarked_calls(&marked), (Vec::new(), 1));
    let separated = format!(
        "    {MARKER}the builder's module\n    let a = 1;\n    let m = r.mutable_at_marked_boundary();\n"
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
        "no call of the capability door found under {}: was it renamed?",
        src.display()
    );
    assert!(
        violations.is_empty(),
        "capability-door calls not directly under a proof marker:\n{}",
        violations.join("\n")
    );
}
