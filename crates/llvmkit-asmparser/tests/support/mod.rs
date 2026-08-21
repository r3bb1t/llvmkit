//! A faithful two-directive subset of FileCheck, shared by the integration
//! tests in this directory.
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

/// Run a fixture's own `CHECK` / `CHECK-NEXT` directives against `text`.
///
/// A faithful two-directive subset of FileCheck, not a paraphrase:
///
/// * `FileCheck::CanonicalizeFile` collapses horizontal-whitespace runs in both
///   buffers; `FileCheck::readCheckFile` ltrims a pattern and
///   `Pattern::parsePattern` rtrims it. Both are reproduced here.
/// * `Pattern::match` is `Buffer.find(FixedStr)` over the *remaining buffer*, and
///   `FileCheckString::Check` resumes at `MatchPos + MatchLen` — a byte position
///   still inside the matched line, so two `CHECK:` directives may match one line.
/// * `FileCheckString::CheckNext` counts newlines in the skipped region with
///   `CountNumNewlinesBetween` and errors "is on the same line as previous match"
///   at zero, "is not on the line after the previous match" otherwise.
/// * `FileCheck::readCheckFile` rejects a leading `-NEXT` directive.
///
/// The byte cursor is load-bearing for `@instructions.win_eh.2`, whose
/// `CHECK: cleanuppad within none []` appears twice and must match two
/// different lines.
///
/// **Only `CHECK` and `CHECK-NEXT` are implemented.** Everything else FileCheck
/// understands is *unimplemented here*, and a fixture needing any of it is
/// *unported*, not narrowed — do not trim the fixture to fit this.
///
/// Nothing else is honoured, in any category — among them no other directive,
/// no modifier, no pattern syntax beyond a fixed substring, no driver option.
/// Read that as a blanket statement, not a list to check against: it stays true
/// as upstream grows FileCheck, which a list would not. Names appear here only
/// where they help, never as a set: `CHECK-SAME`, `CHECK-NOT`, `CHECK-LABEL`,
/// `COM:`, `{LITERAL}`, `{{regex}}`, `[[var]]`, `--implicit-check-not`,
/// `--strict-whitespace`, `--check-prefix`.
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
    assert!(
        !matches!(checks.first(), Some(Check::Next(_))),
        "found 'CHECK-NEXT' without previous 'CHECK: line"
    );
    let haystack = canonicalize_horizontal_whitespace(text);
    let mut cursor = 0usize;
    for check in checks {
        let (raw, is_next) = match check {
            Check::Line(needle) => (needle, false),
            Check::Next(needle) => (needle, true),
        };
        let canonical = canonicalize_horizontal_whitespace(raw);
        let needle = canonical.trim_matches(|c| c == ' ' || c == '\t');
        let found = haystack[cursor..].find(needle).unwrap_or_else(|| {
            let kind = if is_next { "CHECK-NEXT" } else { "CHECK" };
            panic!("{kind}: {needle:?} not found after byte {cursor} in:\n{text}")
        });
        if is_next {
            let newlines = count_newlines_between(&haystack[cursor..cursor + found]);
            assert!(
                newlines != 0,
                "CHECK-NEXT: is on the same line as previous match ({needle:?}) in:\n{text}"
            );
            assert!(
                newlines == 1,
                "CHECK-NEXT: is not on the line after the previous match \
                 ({needle:?}) in:\n{text}"
            );
        }
        cursor += found + needle.len();
    }
}
