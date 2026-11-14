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

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively, sorted. A directory that cannot
/// be read is an error, not an empty scan.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).expect("a source directory reads");
        for entry in entries {
            let path = entry.expect("a directory entry reads").path();
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
/// impl is a `Terminating` one when a code line of the header bounds
/// `S: Terminating`. An impl whose line ends in `{}` opens and closes there.
/// Any other column-0 code line — the impl's closing `}`, a free `fn`, a
/// macro definition or call (`impl_thing!(..)` included), a `mod` — leaves
/// every impl, so a build there counts as outside. Comment lines are neither
/// header nor builds.
fn builds_outside_terminating_impls(source: &str, kinds: &[String]) -> (Vec<usize>, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut outside = Vec::new();
    let mut builds = 0;
    let mut in_terminating_impl = false;
    let mut in_header = false;
    for (index, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        let is_comment = code.starts_with("//");
        let column_zero_code = !line.is_empty()
            && !line.starts_with(char::is_whitespace)
            && !is_comment
            && !line.starts_with('#');
        if line.starts_with("impl<") || line.starts_with("impl ") {
            in_terminating_impl = false;
            in_header = true;
        } else if column_zero_code && !in_header {
            in_terminating_impl = false;
        }
        if in_header && !is_comment {
            if code.starts_with("S: Terminating") || line.contains(" S: Terminating") {
                in_terminating_impl = true;
            }
            let end = line.trim_end();
            if end.ends_with("{}") {
                in_header = false;
                in_terminating_impl = false;
            } else if end.ends_with('{') {
                in_header = false;
            }
        }
        if is_comment {
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

/// Every terminator payload `ir_builder.rs` and the modules under
/// `src/ir_builder/` (recursively) build sits in an impl bounded
/// `S: Terminating`, so a lent `InPass` builder can reach none of them.
/// Positive control: the kind reader finds `Ret` and `Unreachable`; and, in
/// synthetic sources, the scanner flags a build in a `S: Positioning` impl,
/// accepts one in a `S: Terminating` impl, and flags a free helper after a
/// `Terminating` impl closes, after a column-0 `impl_thing!(..)` call whose
/// arguments name `S: Terminating`, and after a one-line `impl .. {}`
/// followed by a comment naming it; and the tree has builds to check, so a
/// rename cannot make the test pass vacuously.
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
    // Each control below passes only on the scanner as it is now; each names
    // the rule an older scanner broke.
    // An impl ends at its closing `}`: a free helper after it is outside.
    let after_close =
        format!("{terminating}fn helper() -> K {{\n    InstructionKindData::Br(p)\n}}\n");
    assert_eq!(
        builds_outside_terminating_impls(&after_close, &kinds),
        (vec![10], 2)
    );
    // A column-0 macro call starting with `impl` opens no header, so a
    // `S: Terminating` among its arguments marks nothing.
    let macro_call = "impl_thing!(\n    S: Terminating,\n);\nfn helper() -> K {\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(macro_call, &kinds),
        (vec![5], 1)
    );
    // A one-line `impl .. {}` closes its own header, so a comment naming
    // `S: Terminating` after it marks nothing.
    let empty_impl = "impl Marker for State {}\n/// for the impls bounded S: Terminating\nfn helper() -> K {\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(empty_impl, &kinds),
        (vec![4], 1)
    );

    let mut files = vec![String::from("ir_builder.rs")];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    files.extend(rust_files(&src.join("ir_builder")).into_iter().map(|path| {
        path.strip_prefix(&src)
            .expect("under src/")
            .to_string_lossy()
            .into_owned()
    }));
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
