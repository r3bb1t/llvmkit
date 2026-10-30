//! A faithful subset of FileCheck, shared by the integration tests in this
//! directory: four directives, and the two pattern forms
//! `utils/update_test_checks.py` writes.
//!
//! Items do not cross integration-test binaries, so a routine used by more than
//! one of them has to live in a module they each `mod`-include. This is the
//! `tests/support/` home `docs/future-work.md` records; the remaining
//! `assert_check_lines` copies it is meant to replace are still open there.
//!
//! Each includer compiles its own copy of this module and uses a subset of it —
//! a fixture whose `CHECK` block writes no `-NEXT` never constructs
//! [`Check::Next`] — so `dead_code` fires on whatever that binary happens not to
//! reach. The declaration is therefore `pub mod support;` with `pub` items: this
//! *is* the API surface the test binaries consume, and saying so is a
//! visibility statement rather than a lint suppression, which the repo forbids.
//! Do not narrow it back to `pub(crate)` without giving every includer a reason
//! to touch every item.
//!
//! Beside the `FileCheck` subset it holds the rejection helpers
//! ([`rejection_at`], [`reported_line_and_column`]) that assert where a
//! diagnostic is anchored as well as what it says.

use std::collections::HashMap;

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_error::ParseError;
use llvmkit_ir::Module;

/// One directive of a ported fixture's `CHECK` block.
pub enum Check<'a> {
    /// `; CHECK: <needle>` — `Pattern::match` searches the *remaining buffer*
    /// from the byte cursor, and `FileCheckString::Check` resumes at
    /// `MatchPos + MatchLen`, a byte position still inside the matched line.
    /// Two `CHECK:` directives may therefore match one output line.
    Line(&'a str),
    /// `; CHECK-NEXT: <needle>` — matches like `Check::Line`, then
    /// `FileCheckString::CheckNext` requires exactly one newline in the
    /// skipped region.
    Next(&'a str),
    /// `; CHECK-SAME: <needle>` — matches like `Check::Line`, then
    /// `FileCheckString::CheckSame` requires no newline in the skipped region.
    Same(&'a str),
    /// `; CHECK-LABEL: <needle>` — `FileCheck::checkInput` finds it first, and
    /// the directives before it, and it, are then matched only in the text up
    /// to the end of that match. It may not define or use a variable.
    Label(&'a str),
}

impl<'a> Check<'a> {
    fn name(&self) -> &'static str {
        match self {
            Check::Line(_) => "CHECK",
            Check::Next(_) => "CHECK-NEXT",
            Check::Same(_) => "CHECK-SAME",
            Check::Label(_) => "CHECK-LABEL",
        }
    }

    fn pattern(&self) -> &'a str {
        match self {
            Check::Line(pattern)
            | Check::Next(pattern)
            | Check::Same(pattern)
            | Check::Label(pattern) => pattern,
        }
    }
}

/// `source`'s own `CHECK` block, in order, so a test compares the fixture's
/// directives rather than a hand-copied list.
pub fn read_check_directives(source: &str) -> Vec<Check<'_>> {
    read_check_directives_with_prefixes(source, &["CHECK"])
}

/// `DefaultCommentPrefixes`: the rest of a line after `RUN:` or `COM:` is a
/// comment, which is why the `--check-prefixes=CHECK,LE` of a `RUN` line names
/// no directive.
const COMMENT_PREFIXES: [&str; 2] = ["COM", "RUN"];

/// `source`'s directives for `prefixes`, in order, as `--check-prefixes` reads
/// them. Mirrors the `FindFirstMatchingPrefix` / `FindCheckType` scan in
/// `FileCheck::readCheckFile`: an occurrence inside a word is not a directive,
/// and the pattern runs to the end of the line.
///
/// A suffix this subset does not implement is a panic rather than a skipped
/// line: a fixture needing one is *unported*, not narrowed.
pub fn read_check_directives_with_prefixes<'a>(
    source: &'a str,
    prefixes: &[&str],
) -> Vec<Check<'a>> {
    source
        .lines()
        .filter_map(|line| directive_in_line(line, prefixes))
        .collect()
}

/// `IsPartOfWord`.
fn is_part_of_word(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// `FindFirstMatchingPrefix`, over one line, which is what it comes to because
/// a directive and a comment each take the rest of theirs.
fn directive_in_line<'a>(line: &'a str, prefixes: &[&str]) -> Option<Check<'a>> {
    let mut scanned = 0;
    loop {
        // The first occurrence of any prefix, the longest at one position.
        let (start, prefix) = prefixes
            .iter()
            .chain(&COMMENT_PREFIXES)
            .filter_map(|prefix| {
                line[scanned..]
                    .find(prefix)
                    .map(|at| (scanned + at, *prefix))
            })
            .min_by_key(|(start, prefix)| (*start, core::cmp::Reverse(prefix.len())))?;
        let after = &line[start + prefix.len()..];
        // Check that the matched prefix isn't a suffix of some other
        // check-like word.
        if !line[..start]
            .chars()
            .next_back()
            .is_some_and(is_part_of_word)
        {
            match check_type(after, prefix, line) {
                CheckType::Directive(check) => return Some(check),
                CheckType::Comment => return None,
                CheckType::None => {}
            }
        }
        // `SkipWord`.
        let word = after.find(|c| !is_part_of_word(c)).unwrap_or(after.len());
        scanned = start + prefix.len() + word;
    }
}

/// What `FindCheckType` makes of the text after a prefix.
enum CheckType<'a> {
    Directive(Check<'a>),
    Comment,
    None,
}

/// `FindCheckType`. The arms it has and this subset does not implement panic;
/// `CheckNone` skips the word, so a `CHECK,LE` is no directive.
fn check_type<'a>(after: &'a str, prefix: &str, line: &str) -> CheckType<'a> {
    if COMMENT_PREFIXES.contains(&prefix) {
        return if after.starts_with(':') {
            CheckType::Comment
        } else {
            CheckType::None
        };
    }
    if let Some(pattern) = after.strip_prefix(':') {
        return CheckType::Directive(Check::Line(pattern));
    }
    let suffix = match after.strip_prefix('-') {
        Some(suffix) => suffix,
        None if after.starts_with(['{', '_']) => {
            panic!("this FileCheck subset does not read the directive in: {line}")
        }
        None => return CheckType::None,
    };
    if let Some(pattern) = suffix.strip_prefix("NEXT:") {
        return CheckType::Directive(Check::Next(pattern));
    }
    if let Some(pattern) = suffix.strip_prefix("SAME:") {
        return CheckType::Directive(Check::Same(pattern));
    }
    if let Some(pattern) = suffix.strip_prefix("LABEL:") {
        return CheckType::Directive(Check::Label(pattern));
    }
    let names_a_directive =
        ["NEXT", "SAME", "NOT", "DAG", "LABEL", "EMPTY"]
            .iter()
            .any(|keyword| {
                suffix
                    .strip_prefix(keyword)
                    .is_some_and(|rest| rest.starts_with([':', '{']))
            });
    assert!(
        !names_a_directive && !suffix.starts_with("COUNT-"),
        "this FileCheck subset does not read the directive in: {line}"
    );
    CheckType::None
}

/// One piece of a parsed pattern.
enum Piece {
    /// A fixed run of text, which upstream passes through `Regex::escape`.
    Text(String),
    /// `.*`. `Pattern::match` compiles with `Regex::Newline`, so it stays on
    /// one line.
    AnyRun,
    /// The start of a `[[NAME:...]]` definition.
    Open,
    /// The end of a `[[NAME:...]]` definition.
    Close(String),
    /// `[[NAME]]`.
    Use(String),
}

/// The regex inside a `{{...}}` block or a `[[NAME:...]]` definition: literal
/// text and `.*`, the only forms this subset reads. Anything else a regex can
/// say is a panic rather than a literal, because reading it as text would
/// silently check something else.
fn parse_regex(regex: &str, pieces: &mut Vec<Piece>) {
    let mut rest = regex;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix(".*") {
            pieces.push(Piece::AnyRun);
            rest = tail;
            continue;
        }
        let end = rest
            .find(|c| ".*+?()[]{}|^$\\".contains(c))
            .unwrap_or(rest.len());
        assert!(
            end != 0,
            "this FileCheck subset reads literal text and `.*`, not: {regex}"
        );
        pieces.push(Piece::Text(rest[..end].to_owned()));
        rest = &rest[end..];
    }
}

/// `Pattern::parsePattern`, for the subset this harness reads. A pattern with
/// neither `{{` nor `[[` is a fixed string. Otherwise it is split into `{{...}}`
/// blocks, `[[...]]` blocks and fixed runs; a `[[[` keeps its first `[` as text,
/// as upstream's "extra '['s" comment says.
fn parse_pattern(pattern: &str) -> Vec<Piece> {
    if pattern.len() < 2 || (!pattern.contains("{{") && !pattern.contains("[[")) {
        return vec![Piece::Text(pattern.to_owned())];
    }
    let mut pieces = Vec::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(block) = rest.strip_prefix("{{") {
            let end = block
                .find("}}")
                .expect("found start of regex string with no end '}}'");
            parse_regex(&block[..end], &mut pieces);
            rest = &block[end + 2..];
            continue;
        }
        if rest.starts_with("[[") && !rest.starts_with("[[[") {
            let block = &rest[2..];
            let end = block
                .find("]]")
                .expect("Invalid substitution block, no ]] found");
            let name = |name: &str| {
                let mut chars = name.chars();
                assert!(
                    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                        && chars.all(|c| c.is_alphanumeric() || c == '_'),
                    "this FileCheck subset reads string variables, not: {name}"
                );
                name.to_owned()
            };
            match block[..end].split_once(':') {
                Some((variable, regex)) => {
                    pieces.push(Piece::Open);
                    parse_regex(regex, &mut pieces);
                    pieces.push(Piece::Close(name(variable)));
                }
                None => pieces.push(Piece::Use(name(&block[..end]))),
            }
            rest = &block[end + 2..];
            continue;
        }
        // Handle fixed string matches.
        // Find the end, which is the start of the next regex.
        let end = (1..rest.len())
            .find(|&at| {
                rest.is_char_boundary(at)
                    && (rest[at..].starts_with("{{") || rest[at..].starts_with("[["))
            })
            .unwrap_or(rest.len());
        pieces.push(Piece::Text(rest[..end].to_owned()));
        rest = &rest[end..];
    }
    pieces
}

/// Whether `pieces` matches `text` at `at`, and where the match ends. `.*` is
/// greedy and backs off, which gives POSIX's answer for this subset: `.` cannot
/// cross a newline, so every run stays on one line and the longest overall match
/// is the one each run's longest extent reaches. `open` is where the definition
/// being read started; definitions do not nest.
fn match_at(
    pieces: &[Piece],
    text: &str,
    at: usize,
    open: usize,
    captures: &mut Vec<(String, String)>,
) -> Option<usize> {
    let Some((piece, rest)) = pieces.split_first() else {
        return Some(at);
    };
    match piece {
        Piece::Text(fixed) => text[at..]
            .starts_with(fixed.as_str())
            .then(|| match_at(rest, text, at + fixed.len(), open, captures))
            .flatten(),
        Piece::AnyRun => {
            let line_end = text[at..]
                .find('\n')
                .map_or(text.len(), |offset| at + offset);
            (at..=line_end)
                .rev()
                .filter(|&end| text.is_char_boundary(end))
                .find_map(|end| match_at(rest, text, end, open, captures))
        }
        Piece::Open => match_at(rest, text, at, at, captures),
        Piece::Close(name) => {
            let end = match_at(rest, text, at, open, captures)?;
            captures.push((name.clone(), text[open..at].to_owned()));
            Some(end)
        }
        Piece::Use(_) => unreachable!("`find_pattern` substitutes every use first"),
    }
}

/// `Pattern::match`: the leftmost match of `pieces` in `text`, as its start and
/// end. A fixed string is `StringRef::find`. Otherwise each `[[NAME]]` is
/// replaced by its value first, and each `[[NAME:...]]` that matched records
/// one. A use of a variable defined earlier on the same line is a back-reference
/// upstream and unimplemented here, so it reads as undefined.
fn find_pattern(
    pieces: &[Piece],
    text: &str,
    variables: &mut HashMap<String, String>,
) -> Option<(usize, usize)> {
    if let [Piece::Text(fixed)] = pieces {
        return text.find(fixed.as_str()).map(|at| (at, at + fixed.len()));
    }
    let substituted: Vec<Piece> = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(fixed) => Piece::Text(fixed.clone()),
            Piece::AnyRun => Piece::AnyRun,
            Piece::Open => Piece::Open,
            Piece::Close(name) => Piece::Close(name.clone()),
            Piece::Use(name) => Piece::Text(
                variables
                    .get(name)
                    .unwrap_or_else(|| panic!("undefined variable: {name}"))
                    .clone(),
            ),
        })
        .collect();
    let mut captures = Vec::new();
    let (start, end) = (0..=text.len())
        .filter(|&start| text.is_char_boundary(start))
        .find_map(|start| {
            captures.clear();
            match_at(&substituted, text, start, start, &mut captures).map(|end| (start, end))
        })?;
    variables.extend(captures);
    Some((start, end))
}

/// Mirrors `FileCheck::CanonicalizeFile`, both halves of its loop body: drop
/// the `\r` of a `\r\n` pair (`if (Ptr <= End - 2 && Ptr[0] == '\r' && Ptr[1]
/// == '\n') continue;`, so a lone trailing `\r` survives), then collapse each
/// run of ' ' / '\t' to a single ' '. Upstream applies it to the check file
/// *and* the input file whenever `--strict-whitespace` is absent, which is the
/// case for every fixture ported here.
pub fn canonicalize_horizontal_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Eliminate trailing dosish `\r`.
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue;
        }
        if c != ' ' && c != '\t' {
            out.push(c);
            continue;
        }
        // Otherwise, add one space and advance over neighboring space.
        out.push(' ');
        while let Some(' ' | '\t') = chars.peek() {
            chars.next();
        }
    }
    out
}

/// Mirrors `CountNumNewlinesBetween`: a `\r\n` or `\n\r` pair is one newline.
fn count_newlines_between(region: &str) -> usize {
    let bytes = region.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' || bytes[i] == b'\r' {
            count += 1;
            if i + 1 < bytes.len()
                && (bytes[i + 1] == b'\n' || bytes[i + 1] == b'\r')
                && bytes[i + 1] != bytes[i]
            {
                i += 1;
            }
        }
        i += 1;
    }
    count
}

/// Run a fixture's own `CHECK` block against `text`.
///
/// A faithful subset of FileCheck, not a paraphrase:
///
/// * `FileCheck::CanonicalizeFile` collapses horizontal-whitespace runs in both
///   buffers; `FileCheck::readCheckFile` ltrims a pattern and
///   `Pattern::parsePattern` rtrims it. Both are reproduced here.
/// * `Pattern::match` searches the *remaining buffer*, and
///   `FileCheckString::Check` resumes at `MatchPos + MatchLen` — a byte position
///   still inside the matched line, so two `CHECK:` directives may match one line.
/// * `FileCheckString::CheckNext` counts newlines in the skipped region with
///   `CountNumNewlinesBetween` and errors "is on the same line as previous match"
///   at zero, "is not on the line after the previous match" otherwise.
/// * `FileCheckString::CheckSame` errors "is not on the same line as the
///   previous match" at any newline.
/// * `FileCheck::checkInput` finds each `CHECK-LABEL` first and matches the
///   directives before it, and it, only in the text up to that match's end.
/// * `FileCheck::readCheckFile` rejects a leading `-NEXT` or `-SAME` directive
///   and a `-LABEL` that defines or uses a variable.
///
/// The byte cursor is load-bearing for `@instructions.win_eh.2`, whose
/// `CHECK: cleanuppad within none []` appears twice and must match two
/// different lines.
///
/// **Four directives and two pattern forms are implemented:** `CHECK`,
/// `CHECK-NEXT`, `CHECK-SAME`, `CHECK-LABEL`, a `{{...}}` block, and a
/// `[[NAME:...]]` definition with its `[[NAME]]` use, whose regex may hold only
/// literal text and `.*`. Everything else FileCheck understands is
/// *unimplemented here*, and a fixture needing any of it is *unported*, not
/// narrowed — do not trim the fixture to fit this.
///
/// Nothing else is honoured, in any category — among them no other directive,
/// no modifier, no other regex, no driver option. Read that as a blanket
/// statement, not a list to check against: it stays true as upstream grows
/// FileCheck, which a list would not. Names appear here only where they help,
/// never as a set: `CHECK-NOT`, `COM:`, `{LITERAL}`, `[[#numeric]]`,
/// `--implicit-check-not`, `--strict-whitespace`, `--check-prefix`.
///
/// Some of what is unimplemented is live in the vendored corpus rather than
/// hypothetical: `test/Assembler/block-labels.ll` carries `--match-full-lines`
/// on its RUN line, and `-check-prefix` and `-DFILE=%s` appear on fixtures under
/// `tests/fixtures/upstream/assembler-corpus/`. Those fixtures are driven by
/// `parser_corpus.rs`'s manifest rather than by this harness, which is why they
/// are not a defect today — and why lifting a needle out of one of them into a
/// `check_directives` call would silently drop the option it depends on.
pub fn check_directives(text: &str, checks: &[Check<'_>]) {
    // `FileCheck::readCheckFile`'s wording, reproduced including its unbalanced
    // quote: `"found '" + UsedPrefix + "-" + Type + "' without previous '" +
    // UsedPrefix + ": line"`.
    if let Some(first @ (Check::Next(_) | Check::Same(_))) = checks.first() {
        panic!("found '{}' without previous 'CHECK: line", first.name());
    }
    let parsed: Vec<(&Check<'_>, Vec<Piece>)> = checks
        .iter()
        .map(|check| {
            let canonical = canonicalize_horizontal_whitespace(check.pattern());
            let pieces = parse_pattern(canonical.trim_matches(|c| c == ' ' || c == '\t'));
            assert!(
                !matches!(check, Check::Label(_))
                    || !pieces
                        .iter()
                        .any(|piece| matches!(piece, Piece::Close(_) | Piece::Use(_))),
                "found 'CHECK-LABEL:' with variable definition or use"
            );
            (check, pieces)
        })
        .collect();

    let haystack = canonicalize_horizontal_whitespace(text);
    let mut buffer = haystack.as_str();
    let mut variables = HashMap::new();
    let (mut i, mut j) = (0, 0);
    loop {
        // `FileCheck::checkInput`: scan to the next `CHECK-LABEL`, whose match
        // ends the region the directives up to it are checked in.
        let mut region = if j == parsed.len() {
            buffer
        } else {
            let (check, pieces) = &parsed[j];
            if !matches!(check, Check::Label(_)) {
                j += 1;
                continue;
            }
            let (_, end) = find_pattern(pieces, buffer, &mut variables).unwrap_or_else(|| {
                panic!("CHECK-LABEL: {:?} not found in:\n{text}", check.pattern())
            });
            let (region, rest) = buffer.split_at(end);
            buffer = rest;
            j += 1;
            region
        };
        while i != j {
            let (check, pieces) = &parsed[i];
            let pattern = check.pattern();
            let name = check.name();
            let (start, end) = find_pattern(pieces, region, &mut variables)
                .unwrap_or_else(|| panic!("{name}: {pattern:?} not found in:\n{text}"));
            let newlines = count_newlines_between(&region[..start]);
            if matches!(check, Check::Next(_)) {
                assert!(
                    newlines != 0,
                    "CHECK-NEXT: is on the same line as previous match ({pattern:?}) in:\n{text}"
                );
                assert!(
                    newlines == 1,
                    "CHECK-NEXT: is not on the line after the previous match \
                     ({pattern:?}) in:\n{text}"
                );
            }
            if matches!(check, Check::Same(_)) {
                assert!(
                    newlines == 0,
                    "CHECK-SAME: is not on the same line as the previous match \
                     ({pattern:?}) in:\n{text}"
                );
            }
            region = &region[end..];
            i += 1;
        }
        if j == parsed.len() {
            break;
        }
    }
}

/// 1-based line and column of `offset` within `src`, the coordinates
/// `SourceMgr::PrintMessage` prints for an `SMLoc` and that upstream's
/// `<stdin>:LINE:COL:` / `<file>:[[@LINE-N]]:COL:` `FileCheck` lines pin.
///
/// Shared rather than duplicated: the corpus harness needs it for `loc=` rows
/// and the routine-anchored diagnostic tests need it for the positions no
/// vendored fixture covers.
pub fn line_and_column(src: &[u8], offset: usize) -> (u32, u32) {
    let consumed = &src[..offset.min(src.len())];
    let line = consumed.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = match consumed.iter().rposition(|byte| *byte == b'\n') {
        Some(newline) => offset - newline,
        None => offset + 1,
    };
    let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    (narrow(line), narrow(column))
}

/// Project a rejection's reported span back onto its source as
/// `(line, column)`, the coordinates upstream's `SourceMgr` prints as
/// `<stdin>:LINE:COL:`.
pub fn reported_line_and_column(source: &[u8], error: &ParseError) -> (u32, u32) {
    let start = usize::try_from(error.loc().start).unwrap_or(usize::MAX);
    line_and_column(source, start)
}

/// Parse `source` as a module that must be rejected, and return the rendered
/// message **and** the `(line, column)` it is anchored at.
///
/// Comparing the text alone is what let a diagnostic carry upstream's exact
/// wording from an unrelated token. `test/Verifier/invalid-label-param.ll`
/// was one: its `CHECK` pins only the message, and llvmkit raised that
/// message from another routine at another token, because its call sites
/// lacked `LLParser::parseParameterList`'s argument-type guard. A caller that
/// does not care about the anchor projects `.0`.
pub fn rejection_at(source: impl AsRef<[u8]>) -> (String, (u32, u32)) {
    let source = source.as_ref();
    let module = Module::dynamic("rejection_at");
    let error = Parser::new(source, &module)
        .expect("lexer primes")
        .parse_module()
        .expect_err("source is rejected");
    (error.to_string(), reported_line_and_column(source, &error))
}
