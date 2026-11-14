//! Ratchet for the loose types `CLAUDE.md`'s "No loose types" rule forbids.
//!
//! Each pattern below is a measurable shape of a loose type: a string error
//! payload, a public tuple return, a public `bool` behaviour flag. The sites
//! that existed when the rule was written are counted in [`CEILINGS`]. The
//! test fails when a count rises, because the new site should have been an
//! enum or a struct, and also when a count falls, so the ceiling is lowered
//! in the same commit and the gain cannot be spent again.
//!
//! What the scan reads, so its reach is not overstated: every `.rs` file
//! under each workspace crate's `src/`, skipping lines whose code starts with
//! `//`. A public signature is the text from `pub fn` to its first `{` or
//! `;`, across lines. Not read: tests outside `src/`, macro-generated code,
//! and loose types the patterns do not spell (a sentinel value, a string
//! field naming a closed set). Those are held by the rule and by review.
//!
//! No upstream counterpart: an llvmkit-specific guard on its own API.

use std::path::{Path, PathBuf};

/// A counted loose-type pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    /// `IrError::InvalidOperation { message }`: an error whose only content
    /// is a string. A new condition gets its own typed variant.
    InvalidOperationSite,
    /// A parser diagnostic built from a string: `ParseError::Message` /
    /// `ParseError::Expected`, directly or through the parser's
    /// `message` / `message_at` / `expected` / `expected_at` helpers.
    StringParseDiagnostic,
    /// A `pub fn` returning a tuple of two or more values, bare or inside
    /// `IrResult` / `Option` / `Result`.
    PublicTupleReturn,
    /// A `pub fn` taking a `bool` parameter, other than a `set_*` setter.
    PublicBoolParameter,
}

impl Pattern {
    const ALL: [Self; 4] = [
        Self::InvalidOperationSite,
        Self::StringParseDiagnostic,
        Self::PublicTupleReturn,
        Self::PublicBoolParameter,
    ];

    /// What to write instead, for the failure message.
    fn instead(self) -> &'static str {
        match self {
            Self::InvalidOperationSite => "a typed IrError variant",
            Self::StringParseDiagnostic => "a typed ParseError variant",
            Self::PublicTupleReturn => "a named struct",
            Self::PublicBoolParameter => "a two-variant enum",
        }
    }
}

/// The site counts at the ceiling, measured by this test on `33b2aaaf` with
/// the rule added. Lower one whenever its count falls; never raise one.
const CEILINGS: [(Pattern, usize); 4] = [
    (Pattern::InvalidOperationSite, 101),
    (Pattern::StringParseDiagnostic, 506),
    (Pattern::PublicTupleReturn, 47),
    (Pattern::PublicBoolParameter, 43),
];

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// The workspace crates' `src/` files.
fn source_files() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&crates) else {
        panic!("the workspace's crates directory is readable");
    };
    for entry in entries.flatten() {
        rust_files(&entry.path().join("src"), &mut files);
    }
    files.sort();
    files
}

/// `source` with every line whose code starts with `//` blanked.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each `pub fn` signature in `code`: its name, parameter text and return
/// text (empty when it returns nothing).
fn public_signatures(code: &str) -> Vec<(&str, &str, &str)> {
    let mut signatures = Vec::new();
    for (start, _) in code.match_indices("pub fn ") {
        let rest = &code[start + "pub fn ".len()..];
        let end = rest.find(['{', ';']).unwrap_or(rest.len());
        let signature = &rest[..end];
        let name_end = signature
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(signature.len());
        let name = &signature[..name_end];
        let Some(open) = signature.find('(') else {
            continue;
        };
        // The parameter list closes at the parenthesis that balances `open`.
        let mut depth = 0usize;
        let mut close = signature.len();
        for (offset, c) in signature[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let parameters = &signature[open..close];
        let returns = signature[close..]
            .split_once("->")
            .map_or("", |(_, returns)| {
                returns.split(" where").next().unwrap_or("")
            });
        signatures.push((name, parameters, returns.trim()));
    }
    signatures
}

/// Whether `returns` is a tuple of two or more values, bare or inside one of
/// the wrappers.
fn returns_tuple(returns: &str) -> bool {
    let mut inner = returns;
    for wrapper in ["IrResult<", "Option<", "Result<"] {
        if let Some(rest) = inner.strip_prefix(wrapper) {
            inner = rest;
        }
    }
    let Some(tuple) = inner.strip_prefix('(') else {
        return false;
    };
    // A comma at the tuple's own depth makes it two or more values.
    let mut depth = 0usize;
    for c in tuple.chars() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' if depth == 0 => return false,
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// The count of each pattern in one file's code.
fn count(path: &Path, code: &str, counts: &mut [usize; 4]) {
    let in_parser = path
        .components()
        .any(|component| component.as_os_str() == "llvmkit-asmparser");
    counts[0] += code.matches("IrError::InvalidOperation {").count();
    if in_parser {
        counts[1] += [
            "ParseError::Message {",
            "ParseError::Expected {",
            ".message(",
            ".message_at(",
            ".expected(",
            ".expected_at(",
        ]
        .iter()
        .map(|needle| code.matches(needle).count())
        .sum::<usize>();
    }
    for (name, parameters, returns) in public_signatures(code) {
        if returns_tuple(returns) {
            counts[2] += 1;
        }
        if !name.starts_with("set_") && parameters.contains(": bool") {
            counts[3] += 1;
        }
    }
}

/// The scanner counts each pattern in synthetic sources that spell it, in
/// one-line and multi-line signatures and behind each return wrapper, and
/// does not count the shapes the rule allows: a `set_*` setter, a unit or
/// one-value return, a tuple inside an iterator type, a commented-out site.
///
/// **No upstream counterpart**: the positive control for
/// [`loose_types_do_not_grow`]'s scanner.
#[test]
fn the_scanner_sees_each_pattern() {
    let ir = Path::new("crates/llvmkit-ir/src/synthetic.rs");
    let parser = Path::new("crates/llvmkit-asmparser/src/synthetic.rs");
    let source = "\
        fn a() { return Err(IrError::InvalidOperation { message: \"x\" }); }\n\
        // IrError::InvalidOperation { message: \"commented out\" }\n\
        pub fn pair(&self) -> (ApInt, bool) {}\n\
        pub fn wrapped(&self) -> IrResult<(Value, BlockId)> {}\n\
        pub fn multi_line(\n    &self,\n) -> Option<(u32, Option<u32>)> {}\n\
        pub fn unit(&self) -> IrResult<()> {}\n\
        pub fn one(&self) -> (ApInt) {}\n\
        pub fn iterator(&self) -> impl Iterator<Item = (u32, u32)> {}\n\
        pub fn ctlz(&self, zero_is_poison: bool) -> u32 {}\n\
        pub fn spread(\n    &self,\n    allow_poison: bool,\n) {}\n\
        pub fn set_dso_local(&self, dso_local: bool) {}\n\
        pub fn returns_bool(&self) -> bool {}\n";
    let mut counts = [0usize; 4];
    count(ir, &code_only(source), &mut counts);
    assert_eq!(counts, [1, 0, 3, 2], "the IR crate's patterns");

    let diagnostics = "\
        fn p(&self) { self.message(\"a\"); self.expected_at(loc, \"b\"); }\n\
        fn q() { ParseError::Message { message: m, loc } }\n\
        fn expected(&self) {}\n";
    let mut counts = [0usize; 4];
    count(parser, &code_only(diagnostics), &mut counts);
    assert_eq!(counts, [0, 3, 0, 0], "the parser's string diagnostics");
}

/// Each loose-type pattern is counted across the workspace's sources and held
/// at its ceiling: a rise is a new loose type, a fall a ceiling to lower.
///
/// **No upstream counterpart**: llvmkit-specific, the guard behind
/// `CLAUDE.md`'s "No loose types" rule.
#[test]
fn loose_types_do_not_grow() {
    let mut counts = [0usize; 4];
    for path in source_files() {
        let Ok(source) = std::fs::read_to_string(&path) else {
            panic!("{} is readable", path.display());
        };
        count(&path, &code_only(&source), &mut counts);
    }
    let mut failures = Vec::new();
    for (index, pattern) in Pattern::ALL.into_iter().enumerate() {
        let ceiling = CEILINGS
            .iter()
            .find(|(counted, _)| *counted == pattern)
            .map_or(0, |(_, ceiling)| *ceiling);
        let found = counts[index];
        if found > ceiling {
            failures.push(format!(
                "{pattern:?}: {found} sites, ceiling {ceiling}. Write {} instead \
                 (CLAUDE.md, \"No loose types\").",
                pattern.instead()
            ));
        } else if found < ceiling {
            failures.push(format!(
                "{pattern:?}: {found} sites, ceiling {ceiling}. Lower the ceiling to {found}."
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
