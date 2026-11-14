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

#[path = "support/source_files.rs"]
mod source_files;
use source_files::rust_files;

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

/// `line` without a trailing `//` comment (the whole line, for a comment
/// line, is empty). It cuts at the first `//`, which is right only when no
/// `//` opens inside a string; [`slashes_in_strings`] is the check the test
/// runs over every scanned file that it does not.
fn without_trailing_comment(line: &str) -> &str {
    match line.find("//") {
        Some(start) => &line[..start],
        None => line,
    }
}

/// The 1-based lines of `source` whose first `//` sits inside a string
/// literal: an odd number of unescaped `"` before it. A line that is a
/// comment from its start is never one.
fn slashes_in_strings(source: &str) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter_map(|(index, line)| {
            let start = line.find("//")?;
            let quotes = line[..start]
                .char_indices()
                .filter(|&(at, c)| c == '"' && !line[..at].ends_with('\\'))
                .count();
            (quotes % 2 == 1).then_some(index + 1)
        })
        .collect()
}

/// Whether `code` bounds `S: Terminating` itself — the whole trait name, so
/// `S: TerminatingExt` does not count.
fn bounds_s_terminating(code: &str) -> bool {
    const BOUND: &str = "S: Terminating";
    code.match_indices(BOUND).any(|(start, _)| {
        let before_ok = code[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after_ok = code[start + BOUND.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        before_ok && after_ok
    })
}

/// The 1-based lines of `source` that build one of `kinds` outside an impl
/// whose header bounds `S: Terminating`, and the number of such builds seen.
///
/// A build is a code line (not a `//` comment) containing
/// `InstructionKindData::Kind(`. An impl starts at a column-0 `impl<` or
/// `impl ` line; its header runs to the first line holding a `{` (so
/// `impl .. {}` and `impl .. { fn f() {} }` end it on their own line), and
/// the impl is a `Terminating` one when a header line bounds `S: Terminating`
/// (the whole trait name). Any other column-0 code line — the impl's closing
/// `}`, a free `fn`, a macro definition or call (`impl_thing!(..)` included),
/// a `mod` — leaves every impl, so a build there counts as outside; a
/// column-0 comment does not. Every line is read without its `//` comment, so
/// a comment line or a trailing comment neither bounds an impl nor hides the
/// `{` that closes a header.
fn builds_outside_terminating_impls(source: &str, kinds: &[String]) -> (Vec<usize>, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut outside = Vec::new();
    let mut builds = 0;
    let mut in_terminating_impl = false;
    let mut in_header = false;
    for (index, line) in lines.iter().enumerate() {
        // The line's code, without a trailing `//` comment: a comment never
        // bounds an impl or closes a header.
        let code = without_trailing_comment(line);
        let is_comment = code.trim().is_empty() && !line.trim().is_empty();
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
        // A comment line's `code` is blank, so it neither bounds the impl nor
        // closes the header.
        if in_header {
            if bounds_s_terminating(code) {
                in_terminating_impl = true;
            }
            // The header ends at the first line holding a `{`, wherever on
            // the line it is: `impl .. {}` and `impl .. { fn f() {} }` end
            // their header on their own line, so the next column-0 item
            // leaves the impl.
            if code.contains('{') {
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
/// Positive control: the kind reader finds `Ret` and `Unreachable`; in
/// synthetic sources the scanner flags a build in a `S: Positioning` impl
/// and accepts one in a `S: Terminating` impl; and ten more controls pin the
/// scanner's six rules — an impl's closing `}` (one control); `impl<` /
/// `impl ` only (one); a header ends at its first `{` (two: `impl .. {}`, a
/// whole body on the header line); comment stripping (three: a comment line,
/// a trailing comment, a `{` before a trailing comment); the whole trait
/// name (two: a longer name after `S`, and before it); a column-0 comment is
/// no item (one). Each control was checked by breaking only the mechanism it
/// names and seeing its assertion fail. The precondition check
/// ([`slashes_in_strings`]) has its own control. And the tree has builds to
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
    // Each control below pins one scanner rule: with that rule removed and
    // the others kept, its assertion fails (each was checked so, removing
    // the rule by hand).
    // Rule: an impl ends at its column-0 closing `}`.
    let after_close =
        format!("{terminating}fn helper() -> K {{\n    InstructionKindData::Br(p)\n}}\n");
    assert_eq!(
        builds_outside_terminating_impls(&after_close, &kinds),
        (vec![10], 2)
    );
    // Rule: only `impl<` / `impl ` opens a header, so a column-0 macro call
    // naming `S: Terminating` among its arguments marks nothing.
    let macro_call = "impl_thing!(\n    S: Terminating,\n);\nfn helper() -> K {\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(macro_call, &kinds),
        (vec![5], 1)
    );
    // Rule: a header ends at the first line holding a `{`: a one-line
    // `impl .. {}` closes its own header, so a following fn's
    // `where S: Terminating` marks nothing ...
    let empty_impl = "impl Marker for State {}\nfn helper<S>() -> K\nwhere\n    S: Terminating,\n{\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(empty_impl, &kinds),
        (vec![6], 1)
    );
    // Rule: every line is read without its `//` comment, so a comment line
    // in a header bounds nothing ...
    let comment_line = "impl<S> X<S>\nwhere\n    S: Positioning,\n    // unlike an impl bounded S: Terminating\n{\n    fn f(&self) {\n        InstructionKindData::Br(p);\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(comment_line, &kinds),
        (vec![7], 1)
    );
    // ... a trailing comment bounds nothing ...
    let trailing_comment = "impl<S> X<S>\nwhere\n    S: Positioning, // unlike S: Terminating\n{\n    fn f(&self) {\n        InstructionKindData::Br(p);\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(trailing_comment, &kinds),
        (vec![6], 1)
    );
    // ... and does not hide the `{` that closes a header.
    let brace_then_comment = "impl<S> X<S> where S: Terminating { // note\n}\nfn helper() -> K {\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(brace_then_comment, &kinds),
        (vec![4], 1)
    );
    // Rule: the bound is the whole trait name.
    let longer_name = "impl<S> X<S>\nwhere\n    S: TerminatingExt,\n{\n    fn f(&self) {\n        InstructionKindData::Br(p);\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(longer_name, &kinds),
        (vec![6], 1)
    );
    // ... on both sides of `S`.
    let longer_before = "impl<S> X<S>\nwhere\n    XS: Terminating,\n{\n    fn f(&self) {\n        InstructionKindData::Br(p);\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(longer_before, &kinds),
        (vec![6], 1)
    );
    // Rule: a column-0 comment inside an impl is no column-0 item, so it
    // does not end the impl.
    let column_zero_comment = "impl<S> X<S>\nwhere\n    S: Terminating,\n{\n// a note at column 0\n    fn f(self) {\n        InstructionKindData::Br(p);\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(column_zero_comment, &kinds),
        (Vec::new(), 1)
    );
    // The precondition check: a `//` inside a string is found; a comment
    // line, and a trailing comment after a closed string, are not.
    assert_eq!(
        slashes_in_strings("let s = \"a//b\";\n// fine\nlet t = \"x\"; // fine\n"),
        vec![1]
    );
    // ... and so does a header line carrying a whole body, so the next free
    // fn is outside the impl.
    let one_line_body = "impl<S> X<S> where S: Terminating { fn f() {} }\nfn helper() -> K {\n    InstructionKindData::Br(p)\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(one_line_body, &kinds),
        (vec![3], 1)
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
        let text = source(file);
        let (outside, seen) = builds_outside_terminating_impls(&text, &kinds);
        builds += seen;
        violations.extend(outside.into_iter().map(|line| format!("{file}:{line}")));
        // The scanner's precondition: no `//` opens inside a string, which
        // `without_trailing_comment` would mistake for a comment and cut a
        // build away with.
        violations.extend(
            slashes_in_strings(&text).into_iter().map(|line| {
                format!("{file}:{line} (`//` inside a string; the scanner would cut it)")
            }),
        );
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
