//! Anti-drift guard for the builder's unchecked positioning door.
//!
//! `IrBuilder::position_at_end_trusting_same_module` positions a builder at a
//! block without comparing modules. The public entries reach it only after a
//! proof — `position_at_end`'s `B: UniqueBrand` bound, or the comparison in
//! `try_position_at_end` and `at_end` — and crate code calls it only for a
//! block it just created or read from its own function. Nothing in the
//! compiler ties a call to its reason, so a new caller handing it a block of
//! another `DynBrand` module would write into the wrong arena with no test
//! failing. This test is that tie: every call sits under a
//! `// Same-module block:` comment saying why the block is the builder's
//! module's, and the test also fails on that marker above anything but a door
//! call, so the marker cannot drift into a general justification.
//!
//! No upstream counterpart: LLVM's `BasicBlock *` carries no module tag.

use std::path::{Path, PathBuf};

/// The door's call spelling.
const DOOR_CALL: &str = ".position_at_end_trusting_same_module(";

/// The comment a call must sit under: a marker this door alone uses, so the
/// crate's general `// Internal:` justification (which sits above many
/// `slot_trusting_same_module` reads) cannot stand in for it. Spelled in two
/// halves so that this file's own spelling is no call site's justification.
const MARKER: &str = concat!("// ", "Same-module block:");

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

/// The 1-based lines of the door calls in `source` that are **not** under a
/// marker, and the number of calls seen.
///
/// A call is a code line (not a `//` comment) containing [`DOOR_CALL`]. It is
/// under a marker when the run of `//` comment lines directly above its
/// statement contains [`MARKER`]. The statement starts at the call's line, or,
/// for a method chain `rustfmt` wrapped, at the nearest line above that does
/// not start with `.`.
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
        // Walk up to the first line of the statement: a line starting with
        // `.` continues the one above.
        let mut statement_start = index;
        while statement_start > 0 && lines[statement_start].trim_start().starts_with('.') {
            statement_start -= 1;
        }
        let marked = lines[..statement_start]
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

/// The 1-based lines of the markers in `source` whose statement is no door
/// call: the marker is reserved for this door, so one above anything else is
/// a misuse that would let the marker stop meaning what this test reads it
/// as. A marker's statement is the first code line below its comment run,
/// with the `.`-led lines `rustfmt` wraps a chain onto.
fn orphan_markers(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut orphans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with(MARKER) {
            continue;
        }
        let mut below = lines[index + 1..]
            .iter()
            .map(|line| line.trim_start())
            .skip_while(|line| line.starts_with("//"));
        let Some(first) = below.next() else {
            orphans.push(index + 1);
            continue;
        };
        let mut statement = vec![first];
        statement.extend(below.take_while(|line| line.starts_with('.')));
        if !statement.iter().any(|line| line.contains(DOOR_CALL)) {
            orphans.push(index + 1);
        }
    }
    orphans
}

/// Every call of `IrBuilder::position_at_end_trusting_same_module` in
/// `crates/llvmkit-ir/src` sits under a `// Same-module block:` comment, and
/// every such comment sits above a door call — so the marker is this door's
/// alone because the test holds it so, not by convention. Positive control:
/// the scanner flags an unmarked call and one under only the crate's general
/// `// Internal:` comment, accepts a marked one on one line and wrapped, and
/// flags a marker above a statement that is no door call, in synthetic
/// sources; and the tree has calls to check, so a rename cannot make the test
/// pass vacuously.
///
/// No upstream counterpart: llvmkit-specific regression for the unchecked
/// positioning door (`docs/design/layout-token-design.md` decisions 8 and 10).
#[test]
fn every_trusting_position_call_sits_under_a_same_module_marker() {
    let unmarked = "fn f() {\n    // a comment, but no marker\n    let b = b.position_at_end_trusting_same_module(bb);\n}\n";
    assert_eq!(unmarked_calls(unmarked), (vec![3], 1));
    let general = "fn f() {\n    // Internal: `x` was admitted above.\n    let b = b.position_at_end_trusting_same_module(bb);\n}\n";
    assert_eq!(unmarked_calls(general), (vec![3], 1));
    let marked = format!(
        "fn f() {{\n    {MARKER} `bb` was created here.\n    let b = b.position_at_end_trusting_same_module(bb);\n}}\n"
    );
    assert_eq!(unmarked_calls(&marked), (Vec::new(), 1));
    let wrapped = format!(
        "fn f() {{\n    {MARKER} `bb` was created here.\n    let b = IrBuilder::new(key)\n        .position_at_end_trusting_same_module(bb)\n        .ret_void();\n}}\n"
    );
    assert_eq!(unmarked_calls(&wrapped), (Vec::new(), 1));
    assert_eq!(orphan_markers(&wrapped), Vec::<usize>::new());
    let orphan = format!("fn f() {{\n    {MARKER} not a door call.\n    let s = x.slot();\n}}\n");
    assert_eq!(orphan_markers(&orphan), vec![2]);

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
        for line in orphan_markers(&source) {
            violations.push(format!(
                "{}:{line} (marker above no door call)",
                file.display()
            ));
        }
    }
    assert!(
        total > 0,
        "no call of the trusting door found under {}: was it renamed?",
        src.display()
    );
    assert!(
        violations.is_empty(),
        "trusting-door calls not under a `// Same-module block:` comment:\n{}",
        violations.join("\n")
    );
}
