//! Blame — whether a failure is llvmkit's bug — is a value, not words inside a
//! message.
//!
//! **llvmkit-specific in shape, ported in substance.** Upstream draws this exact
//! line: `llvm/include/llvm/Support/ErrorHandling.h` deprecates
//! `report_fatal_error`'s `gen_crash_diag` flag in favour of
//! `reportFatalInternalError` (a bug in LLVM; ask for a bug report) and
//! `reportFatalUsageError` (not a bug; invalid input, environment limits, or
//! unimplemented functionality). Upstream aborts where llvmkit returns, so there
//! is no upstream error *value* to port and no upstream test to mirror — only
//! the distinction itself.
//!
//! What the two prose scans read, so their reach is not overstated: the string
//! literal of every `#[error(...)]` attribute in `src/error.rs`, whether it sits
//! on the attribute's own line or the next; and every string literal written
//! straight after a `message:` field anywhere under this crate's `src/`, which
//! is where `IrError::InvalidOperation`'s `&'static str` lives and where the
//! `insert_phi` defect was. Both follow Rust's `\`-newline continuation, so a
//! phrase split across two source lines is still read as one phrase. Not read:
//! text built by `format!`; literals passed positionally (the verifier's
//! `Check` literals); raw-string literals; a literal bound to a local first and
//! handed over by field shorthand, as `derived_types.rs`'s `check_params` does
//! with `InvalidOperation { message }`; and the parser crate's messages.

use std::path::{Path, PathBuf};

use llvmkit_ir::{Blame, BrandError, IrError};

const ERROR_RS: &str = include_str!("../src/error.rs");

/// Stems that spell a blame class in prose, matched anywhere in a lowercased
/// literal. `"internal invariant"` and `"llvmkit bug"` are subsumed by
/// `"invariant"` and `"llvmkit"`. Bare `"internal"` is deliberately absent: it
/// is a linkage name, and the verifier's ifunc-linkage message lists it.
const BLAME_STEMS: &[&str] = &[
    "invariant",
    "llvmkit",
    "unreachable",
    "impossible",
    "internal error",
    "should never",
    "never happen",
    "cannot happen",
    "caller supplied",
];

/// Stems matched only as a whole word, because as a substring they occur
/// inside ordinary words — `bug` inside `debug`.
const BLAME_WORDS: &[&str] = &["bug"];

/// Every `IrError` says whose fault it is, and the compiler enforces that.
///
/// This test cannot fail for a *missing* classification — omitting a variant
/// from `blame()`'s match is a compile error, which is the point. What it pins
/// is that both answers are reachable, so a `blame()` that returned one
/// constant would not pass, and that each variant whose every construction
/// site is beyond a caller's reach answers `LlvmkitInvariant`.
#[test]
fn both_blame_answers_are_reachable() {
    assert_eq!(
        IrError::InvalidIntegerWidth { bits: 0 }.blame(),
        Blame::UsageError,
        "a width outside LLVM's own bound is not an llvmkit bug"
    );
    assert_eq!(
        IrError::Brand(BrandError::InUse { brand: "b" }).blame(),
        Blame::UsageError,
        "claiming a live brand is a caller mistake"
    );
    for (error, why) in [
        (
            IrError::UnknownMetadataSlot { index: 0, len: 0 },
            "a slot past the arena under a matching module tag: no caller can mint such an id",
        ),
        (
            IrError::AnalysisResultMissingAfterCaching { name: "A" },
            "the manager could not read back a result it had just cached",
        ),
        (
            IrError::AliaseeTypeChangedBeforeBuild,
            "a same-module aliasee's type cannot change: nothing writes a value's type",
        ),
        (
            IrError::IfuncResolverTypeChangedBeforeBuild,
            "a same-module resolver's type cannot change: nothing writes a value's type",
        ),
    ] {
        assert_eq!(error.blame(), Blame::LlvmkitInvariant, "{why}");
    }
}

/// No message repeats in words what `blame()` now states as a value.
///
/// With blame typed, prose that also spells it is duplication that can drift
/// out of step with the type — the `insert_phi` message was wrong for the life
/// of the message precisely because nothing could compare the two.
///
/// Reads each `#[error(...)]` attribute's literal, not each line holding
/// `#[error(`: an attribute whose literal starts on the next line would
/// otherwise be skipped whole. The count check proves every non-transparent
/// attribute yielded a literal, so a reader that skipped one cannot pass.
#[test]
fn no_message_repeats_what_blame_states() {
    let messages = literals_after(ERROR_RS, "#[error(");
    let attributes = ERROR_RS.matches("#[error(").count();
    let transparent = ERROR_RS.matches("#[error(transparent)]").count();
    assert_eq!(
        messages.len(),
        attributes - transparent,
        "every non-transparent `#[error]` attribute must yield its literal"
    );
    let offenders: Vec<String> = messages
        .iter()
        .filter(|(_, literal)| spells_blame(literal))
        .map(|(line, literal)| format!("src/error.rs:{line}: {literal}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a message spells its blame class in prose, where `IrError::blame()` \
         already states it as a value that cannot drift:\n{}",
        offenders.join("\n")
    );
}

/// No `message:` payload repeats in words what `blame()` states as a value.
///
/// `IrError::InvalidOperation` carries its text as a `&'static str` written at
/// the construction site, far from `error.rs`, and that is where
/// `FnReshape::insert_phi` wrote `"... (internal invariant)"`. The
/// `#[error]` scan above cannot see such a site, so this one walks every source
/// file in the crate. No upstream counterpart: `report_fatal_error` takes a
/// `Twine`, and there is no message payload beside a typed classification to
/// compare.
#[test]
fn no_message_payload_repeats_what_blame_states() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files_under(&source_root, &mut files);
    files.sort_unstable();

    let mut offenders = Vec::new();
    let mut pass_context_literals = 0;
    for file in &files {
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("reading {}: {error}", file.display()));
        let literals = literals_after(&source, "message:");
        if file.ends_with("pass_context.rs") {
            pass_context_literals = literals.len();
        }
        offenders.extend(
            literals
                .iter()
                .filter(|(_, literal)| spells_blame(literal))
                .map(|(line, literal)| format!("{}:{line}: {literal}", file.display())),
        );
    }
    assert!(
        pass_context_literals > 0,
        "the walk must reach `pass_context.rs` and read its `message:` literals, \
         or this scan is blind where the `insert_phi` defect lived"
    );
    assert!(
        offenders.is_empty(),
        "a `message:` payload spells its blame class in prose, where \
         `IrError::blame()` already states it as a value that cannot drift:\n{}",
        offenders.join("\n")
    );
}

/// The phrase matcher both scans share reads what it claims to: the prose
/// spelling of each `Blame` variant, case-insensitively, and `bug` only as a
/// whole word. Without this, a matcher that silently matched nothing would
/// pass both scans. llvmkit-specific, no upstream counterpart.
#[test]
fn the_blame_matcher_reads_what_it_claims() {
    for spelled in [
        "bad phi (llvmkit invariant)",
        "bad phi (Internal Invariant)",
        "(invariant violated)",
        "(bug in llvmkit)",
        "a bug.",
        "unreachable state",
        "Impossible",
    ] {
        assert!(spells_blame(spelled), "must flag {spelled:?}");
    }
    for plain in [
        "debug info",
        "bugged",
        "private, internal, linkonce",
        "type mismatch",
    ] {
        assert!(!spells_blame(plain), "must not flag {plain:?}");
    }
}

/// `Blame` is `Copy` and cheap, so reading it never forces a clone of the error.
#[test]
fn blame_is_a_copy_value() {
    fn requires_copy<T: Copy + core::fmt::Debug + Eq + core::hash::Hash>() {}
    requires_copy::<Blame>();
}

fn spells_blame(literal: &str) -> bool {
    let lowered = literal.to_lowercase();
    BLAME_STEMS.iter().any(|stem| lowered.contains(stem))
        || BLAME_WORDS
            .iter()
            .any(|word| contains_whole_word(&lowered, word))
}

/// `word` occurs in `text` with no letter, digit or underscore on either side.
fn contains_whole_word(text: &str, word: &str) -> bool {
    let is_word_character = |character: char| character.is_alphanumeric() || character == '_';
    text.match_indices(word).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + word.len()..].chars().next();
        !before.is_some_and(is_word_character) && !after.is_some_and(is_word_character)
    })
}

/// Every `.rs` file under `directory`, recursively. A directory that cannot be
/// read fails the test rather than shrinking the scan to nothing.
fn rust_files_under(directory: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("listing {}: {error}", directory.display()))
            .path();
        if path.is_dir() {
            rust_files_under(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

/// The string literal opening straight after each `marker` in `source`, with
/// Rust's `\`-newline continuation applied, paired with the 1-based line the
/// marker is on. A marker followed by anything but a quote —
/// `#[error(transparent)]`, `message: format!(..)`, a field declaration —
/// yields nothing.
fn literals_after(source: &str, marker: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (offset, _) in source.match_indices(marker) {
        let line = source[..offset].matches('\n').count() + 1;
        let Some(body) = source[offset + marker.len()..]
            .trim_start()
            .strip_prefix('"')
        else {
            continue;
        };
        let mut literal = String::new();
        let mut characters = body.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '"' => break,
                '\\' => match characters.peek() {
                    // `\` at end of line drops the line break and the next
                    // line's leading whitespace.
                    Some('\r' | '\n') => {
                        while characters.next_if(|next| next.is_whitespace()).is_some() {}
                    }
                    // Any other escape is kept as written, and an escaped
                    // quote does not end the literal.
                    Some(_) => {
                        literal.push(character);
                        literal.extend(characters.next());
                    }
                    None => {}
                },
                other => literal.push(other),
            }
        }
        found.push((line, literal));
    }
    found
}
