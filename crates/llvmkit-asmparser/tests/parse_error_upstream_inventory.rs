//! Census of llvmkit's parser diagnostics against the vendored upstream sources.
//!
//! `ParseError::Expected` and `ParseError::Message` carry `Cow<'static, str>`,
//! so a diagnostic upstream never emits is spellable and invisible in review.
//! This test measures how many exist today, in both directions, and prints
//! them. It asserts nothing: its output is the input to the decision about
//! whether to convert the remaining call sites to variants, per
//! `docs/design/parse-error-algebraic-design.md`.
//!
//! `#[ignore]`d because it is a measurement, not a gate. The gate arrives with
//! the variants, which can be enumerated; a `Cow` cannot.
//!
//! Both `.cpp` files are vendored under this crate's `tablegen/` (tracked,
//! unlike `orig_cpp/`), matching `attribute_td_drift.rs`. `LLLexer.cpp` is
//! included because the lexer owns eight of our diagnostic texts; a census
//! reading only `LLParser.cpp` would report all eight as inventions.

use std::collections::{BTreeMap, BTreeSet};

const LLPARSER_CPP: &str = include_str!("../tablegen/llvm-22.1.4/lib/AsmParser/LLParser.cpp");
const LLLEXER_CPP: &str = include_str!("../tablegen/llvm-22.1.4/lib/AsmParser/LLLexer.cpp");
const LL_PARSER_RS: &str = include_str!("../src/ll_parser.rs");

/// Read the double-quoted literal beginning at `chars[open]`, returning its
/// decoded text and the index just past the closing quote.
///
/// Escapes keep the escaped character and drop the backslash, so the label
/// `c\"...\" constant` survives as one string. A scan that stops at the first
/// bare quote truncates it, which is what makes a naive extractor undercount.
fn read_literal(chars: &[char], open: usize) -> (String, usize) {
    let mut text = String::new();
    let mut i = open + 1;
    while i < chars.len() && chars[i] != '"' {
        if chars[i] == '\\' && i + 1 < chars.len() {
            text.push(chars[i + 1]);
            i += 2;
        } else {
            text.push(chars[i]);
            i += 1;
        }
    }
    (text, i + 1)
}

/// Index of the delimiter matching the one at `open`, skipping string literals
/// so a bracket inside a message cannot close the construct.
fn matching(chars: &[char], open: usize, opener: char, closer: char) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            i = read_literal(chars, i).1;
            continue;
        }
        if c == opener {
            depth += 1;
        } else if c == closer {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
        i += 1;
    }
    chars.len()
}

/// The first string literal inside the construct opened at `open`, if any.
///
/// `None` is the correct answer for a call passing a variable rather than a
/// literal, such as `builder_err(op.mnemonic(), e)`.
fn first_literal(chars: &[char], open: usize, opener: char, closer: char) -> Option<String> {
    let end = matching(chars, open, opener, closer);
    let mut i = open + 1;
    while i < end {
        if chars[i] == '"' {
            return Some(read_literal(chars, i).0);
        }
        i += 1;
    }
    None
}

/// Every occurrence of `marker`, as the index of the delimiter that ends it.
fn sites(chars: &[char], marker: &str) -> Vec<usize> {
    let needle: Vec<char> = marker.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] == needle[..] {
            out.push(i + needle.len() - 1);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// What one construction family contributes to the census.
#[derive(Default)]
struct Family {
    /// Distinct rendered texts, or `format!` templates.
    texts: BTreeSet<String>,
    /// Call sites found.
    sites: usize,
    /// Sites whose text is not an inline literal -- a `const`, a helper's
    /// parameter, or a function call. **Not necessarily unmeasured**: the text
    /// usually appears at some other site that does carry it. They are counted
    /// and reported rather than dropped, because a diagnostic whose text is
    /// computed elsewhere is exactly the kind the drift test cannot see either.
    computed: usize,
}

/// Our diagnostics, keyed by construction family.
///
/// **Known limitation, deliberate.** Only the *first* literal in each call is
/// taken, so a call whose argument is a conditional -- `self.expected(if x
/// { "a" } else { "b" })` -- contributes `"a"` and not `"b"`. Widening this to
/// collect every literal would instead over-collect from nested calls. The
/// residue is small and shows up as a lower distinct-count than site-count;
/// check the conditional sites by hand when reconciling.
fn our_diagnostics() -> BTreeMap<&'static str, Family> {
    let chars: Vec<char> = LL_PARSER_RS.chars().collect();
    let mut out: BTreeMap<&'static str, Family> = BTreeMap::new();

    let mut collect = |family: &'static str,
                       marker: &str,
                       opener: char,
                       closer: char,
                       prefix: &str,
                       suffix: &str| {
        for open in sites(&chars, marker) {
            let entry = out.entry(family).or_default();
            entry.sites += 1;
            match first_literal(&chars, open, opener, closer) {
                Some(text) => {
                    entry.texts.insert(format!("{prefix}{text}{suffix}"));
                }
                None => entry.computed += 1,
            }
        }
    };

    // `expected` renders with the prefix its variant prepends.
    collect("expected", "self.expected(", '(', ')', "expected ", "");
    collect("expected", "self.expected_at(", '(', ')', "expected ", "");
    // `message` renders verbatim.
    collect("message", "self.message(", '(', ')', "", "");
    collect("message", "self.message_at(", '(', ')', "", "");
    // Direct constructions, brace-delimited. These also pick up the `format!`
    // template, which is the first literal inside the braces.
    collect(
        "direct",
        "ParseError::Expected {",
        '{',
        '}',
        "expected ",
        "",
    );
    collect("direct", "ParseError::Message {", '{', '}', "", "");
    // The builder family, bucketed apart: absent upstream by design.
    collect(
        "builder",
        "self.builder_err(",
        '(',
        ')',
        "expected valid ",
        ": <IrError>",
    );
    collect(
        "builder",
        "self.builder_err_at(",
        '(',
        ')',
        "expected ",
        ": <IrError>",
    );

    out
}

/// Upstream's diagnostic literals, with adjacent literals joined.
///
/// C concatenates two adjacent literals into one string, so a scan treating
/// them as two reports both halves as separate messages and matches neither.
fn upstream_diagnostics() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for source in [LLPARSER_CPP, LLLEXER_CPP] {
        let chars: Vec<char> = source.chars().collect();
        for marker in ["error(", "tokError(", "LexError("] {
            for open in sites(&chars, marker) {
                let end = matching(&chars, open, '(', ')');
                let mut i = open + 1;
                while i < end {
                    if chars[i] != '"' {
                        i += 1;
                        continue;
                    }
                    let (mut joined, mut next) = read_literal(&chars, i);
                    loop {
                        let mut probe = next;
                        while probe < end && chars[probe].is_whitespace() {
                            probe += 1;
                        }
                        if probe < end && chars[probe] == '"' {
                            let (more, after) = read_literal(&chars, probe);
                            joined.push_str(&more);
                            next = after;
                        } else {
                            break;
                        }
                    }
                    if !joined.is_empty() {
                        out.insert(joined);
                    }
                    i = next;
                }
            }
        }
    }
    out
}

/// Exact, parameterised (ours is a prefix upstream extends), or absent.
fn bucket(ours: &str, upstream: &BTreeSet<String>) -> &'static str {
    if upstream.contains(ours) {
        return "exact";
    }
    let head = ours.split('{').next().unwrap_or(ours).trim_end();
    if head.len() >= 8 && upstream.iter().any(|u| u.starts_with(head)) {
        return "parameterised";
    }
    "absent"
}

/// llvmkit-specific (**no upstream counterpart**): LLVM has no error enum, so
/// there is nothing upstream to port this census from. It exists to measure
/// llvmkit's own drift from the diagnostic text of `LLParser.cpp` and
/// `LLLexer.cpp`.
#[test]
#[ignore = "measurement, not a gate; run with --ignored -- --nocapture"]
fn parse_error_text_inventory() {
    let ours = our_diagnostics();
    let upstream = upstream_diagnostics();
    println!("upstream literals: {}", upstream.len());

    let mut invented_total = 0usize;
    let mut computed_total = 0usize;
    for (family, data) in &ours {
        let mut by_bucket: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
        for text in &data.texts {
            by_bucket
                .entry(bucket(text, &upstream))
                .or_default()
                .push(text);
        }
        let absent = by_bucket.get("absent").map_or(0, Vec::len);
        if *family != "builder" {
            invented_total += absent;
        }
        computed_total += data.computed;
        println!(
            "\n=== {family}: {} sites | {} distinct | exact {} | parameterised {} | absent {absent} | text computed elsewhere {} ===",
            data.sites,
            data.texts.len(),
            by_bucket.get("exact").map_or(0, Vec::len),
            by_bucket.get("parameterised").map_or(0, Vec::len),
            data.computed,
        );
        for text in by_bucket.get("absent").into_iter().flatten() {
            println!("  ABSENT  {text}");
        }
    }

    // Upstream texts we render nowhere. Coarse: one of our parameterised
    // messages will not contain upstream's full text, so this over-reports. It
    // is a starting list for the drift test's NOT_YET_PORTED, not a verdict.
    let all_ours: BTreeSet<&String> = ours.values().flat_map(|f| &f.texts).collect();
    let missing: Vec<&String> = upstream
        .iter()
        .filter(|u| !all_ours.iter().any(|o| o.starts_with(u.as_str())))
        .collect();
    println!(
        "\n=== upstream texts with no match of ours: {} ===",
        missing.len()
    );
    for text in &missing {
        println!("  MISSING {text}");
    }

    println!(
        "\n=== INVENTED (absent upstream, excluding the builder family): {invented_total} ==="
    );
    println!("That number is the go/no-go input. The builder family is excluded because");
    println!("upstream asserts where we diagnose, so its labels are absent by design.");
    println!("\n{computed_total} sites take their text from a const, a parameter or a call");
    println!("rather than an inline literal. Most reuse a text counted at another site;");
    println!("reconcile them by hand before treating INVENTED as complete.");
}
