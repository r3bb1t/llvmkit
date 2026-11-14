//! Anti-drift guard for the builder's terminator law.
//!
//! A builder a function pass's context lends (`InPass`) must not emit a
//! terminator. The compiler holds that for every terminator build in the
//! impls bounded `S: Terminating`, and for every build that takes its block
//! through `into_block_to_terminate`. Nothing in the compiler stops a new
//! `&self` build from appending a terminator payload through the shared
//! `append_instruction` in a `S: Positioning` impl, which a lent builder
//! reaches. This test is that tie: in `ir_builder.rs` and its child modules
//! (`src/ir_builder/`), every construction of a terminator payload sits in an
//! impl whose header bounds `S: Terminating`. A type-level form — an
//! `append_terminator` bounded `S: Terminating` as the only door for
//! terminator payloads — needs the payload's terminator-ness in its type, and
//! `InstructionKindData` is one enum whose variants share payload types, so
//! the scan is the guard until the payload is split
//! (`docs/future-work.md`).
//!
//! The terminator kinds are read from `InstructionKindData::is_terminator` in
//! `instruction.rs`, so a new terminator kind is covered without editing this
//! file. No upstream counterpart: LLVM's `IRBuilder` has no positioning
//! typestate.

use std::path::Path;

/// The source of `relative`, under this crate's `src/`.
fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative);
    std::fs::read_to_string(&path).expect("source file reads")
}

/// The variant names `InstructionKindData::is_terminator` matches, read from
/// its `matches!` arms (`Self::Ret(_)`, `| Self::Br(_)`, …).
fn terminator_kinds(instruction_rs: &str) -> Vec<String> {
    let start = instruction_rs
        .find("pub(super) fn is_terminator(&self) -> bool {")
        .expect("`InstructionKindData::is_terminator` is in instruction.rs");
    let body = &instruction_rs[start..];
    let end = body.find("\n    }\n").expect("the fn body ends");
    body[..end]
        .split("Self::")
        .skip(1)
        .filter_map(|arm| arm.split('(').next())
        .map(str::to_owned)
        .collect()
}

/// The 1-based lines of `source` that build one of `kinds` outside an impl
/// whose header bounds `S: Terminating`, and the number of such builds seen.
///
/// A build is a code line (not a `//` comment) containing
/// `InstructionKindData::Kind(`. An impl starts at a column-0 `impl<` or
/// `impl ` line; its header runs to the first line ending in `{`, and the
/// impl is a `Terminating` one when the header bounds `S: Terminating`. Any
/// other column-0 code line — the impl's closing `}`, a free `fn`, a macro
/// definition or call, a `mod` — leaves every impl, so a build there counts
/// as outside.
fn builds_outside_terminating_impls(source: &str, kinds: &[String]) -> (Vec<usize>, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut outside = Vec::new();
    let mut builds = 0;
    let mut in_terminating_impl = false;
    let mut in_header = false;
    for (index, line) in lines.iter().enumerate() {
        let column_zero_code =
            !line.is_empty() && !line.starts_with(char::is_whitespace) && !line.starts_with("//");
        if line.starts_with("impl<") || line.starts_with("impl ") {
            in_terminating_impl = false;
            in_header = true;
        } else if column_zero_code && !line.starts_with('#') && !in_header {
            in_terminating_impl = false;
        }
        if in_header {
            if line.trim() == "S: Terminating," || line.contains(" S: Terminating") {
                in_terminating_impl = true;
            }
            if line.trim_end().ends_with('{') {
                in_header = false;
            }
        }
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let builds_terminator = kinds
            .iter()
            .any(|kind| code.contains(&format!("InstructionKindData::{kind}(")));
        if builds_terminator {
            builds += 1;
            if !in_terminating_impl {
                outside.push(index + 1);
            }
        }
    }
    (outside, builds)
}

/// Every terminator payload `ir_builder.rs` builds sits in an impl bounded
/// `S: Terminating`, so a lent `InPass` builder can reach none of them.
/// Positive control: the kind reader finds `Ret` and `Unreachable`, and the
/// scanner flags a build in a `S: Positioning` impl and accepts one in a
/// `S: Terminating` impl, in synthetic sources; and the tree has builds to
/// check, so a rename cannot make the test pass vacuously.
///
/// No upstream counterpart: llvmkit-specific regression for the lent-builder
/// terminator law (`docs/design/layout-token-design.md` decisions 9 and 10).
#[test]
fn every_terminator_payload_is_built_in_a_terminating_impl() {
    let kinds = terminator_kinds(&source("instruction.rs"));
    assert!(
        kinds.iter().any(|kind| kind == "Ret") && kinds.iter().any(|kind| kind == "Unreachable"),
        "the terminator kinds were not read: {kinds:?}"
    );

    // Positive control: the scanner itself.
    let positioning = "impl<S> X<S>\nwhere\n    S: Positioning,\n{\n    fn f(&self) {\n        self.append_instruction(t, InstructionKindData::Ret(p), \"\");\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(positioning, &kinds),
        (vec![6], 1)
    );
    let terminating = "impl<S> X<S>\nwhere\n    S: Terminating,\n{\n    fn f(self) {\n        self.append_instruction(t, InstructionKindData::Br(p), \"\");\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(terminating, &kinds),
        (Vec::new(), 1)
    );
    // A free helper after a `Terminating` impl closes is outside it, and so
    // is one after a column-0 macro call that starts with `impl`.
    let after_close = format!(
        "{terminating}fn helper() -> K {{\n    InstructionKindData::Br(p)\n}}\nimpl_thing!(a, b);\nfn other() -> K {{\n    InstructionKindData::Ret(p)\n}}\n"
    );
    assert_eq!(
        builds_outside_terminating_impls(&after_close, &kinds),
        (vec![10, 14], 3)
    );

    let mut files = vec![String::from("ir_builder.rs")];
    let children = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ir_builder");
    let mut child_names: Vec<String> = std::fs::read_dir(&children)
        .expect("src/ir_builder/ reads")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".rs"))
        .map(|name| format!("ir_builder/{name}"))
        .collect();
    child_names.sort();
    files.extend(child_names);
    let mut builds = 0;
    let mut violations = Vec::new();
    for file in &files {
        let (outside, seen) = builds_outside_terminating_impls(&source(file), &kinds);
        builds += seen;
        violations.extend(outside.into_iter().map(|line| format!("{file}:{line}")));
    }
    assert!(
        builds > 0,
        "no terminator build found in {files:?}: was it renamed?"
    );
    assert!(
        violations.is_empty(),
        "terminator payloads built outside a `S: Terminating` impl: {violations:?}"
    );
}
