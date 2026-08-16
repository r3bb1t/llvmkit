//! Drift lock: the lexer's keyword and token tables against the vendored
//! `LLLexer.cpp` and `LLToken.h`.
//!
//! The fifth of the family, and the one that covers what the other four do
//! not. `attribute_td_drift.rs` guards `Attributes.td`, `calling_conv_drift.rs`
//! guards `CallingConv.h`, `dwarf_def_drift.rs` guards `Dwarf.def` /
//! `DebugInfoFlags.def`, `fixed_metadata_kinds_drift.rs` guards
//! `FixedMetadataKinds.def` — between them the *vocabularies* a few of the
//! lexer's word families carry. Nothing guarded the families themselves: the
//! `KEYWORD` / `TYPEKEYWORD` / `INSTKEYWORD` macro tables in
//! `LLLexer::LexIdentifier`, its `DWKEYWORD` / `DBGRECORDTYPEKEYWORD` prefix
//! families, the hand-written exact-word tails (`DIFlag`, `DISPFlag`, `CSK_`,
//! emission kind, name-table kind, fixed-point kind), or the punctuation arms
//! of `LLLexer::LexToken`.
//!
//! Both directions matter, and they fail differently.
//!
//! * A spelling upstream has and llvmkit lacks makes llvmkit **reject** IR
//!   `llvm-as` accepts. `LLLexer::LexIdentifier`'s tail is
//!   `CurPtr = TokStart+1; return lltok::Error;`, and llvmkit's is the same
//!   `Token::Error`, so the rejection is loud — but it is a rejection.
//! * A spelling llvmkit has and upstream does not makes llvmkit **accept** IR
//!   `llvm-as` rejects, silently.
//! * A spelling both have but in different families — a `KEYWORD` read as a
//!   `TYPEKEYWORD`, say — is the silent one that no round-trip catches.
//!
//! So every family is checked forward (upstream spelling to llvmkit token
//! *family*) and the whole set is checked backward against
//! [`NON_UPSTREAM_KEYWORDS`].
//!
//! `LLLexer.cpp` and `LLToken.h` are vendored under this crate's `tablegen/`
//! for the reason the other four give: `orig_cpp/` is gitignored, so a test
//! that reads it passes locally and fails in CI.
//!
//! llvmkit-specific drift guard (no upstream counterpart — upstream's lexer
//! and token enum both `#include` the same generated `Attributes.inc` and hold
//! the rest in one `LexIdentifier` body, so its tables structurally cannot
//! disagree). The vendored sources are the anchor (D11).

use std::collections::BTreeSet;

use llvmkit_asmparser::ll_lexer::Lexer;
use llvmkit_asmparser::ll_token::Token;

const LLLEXER_CPP: &str = include_str!("../tablegen/llvm-22.1.4/lib/AsmParser/LLLexer.cpp");
const LLTOKEN_H: &str = include_str!("../tablegen/llvm-22.1.4/include/llvm/AsmParser/LLToken.h");
const ATTRIBUTES_TD: &str = include_str!("../tablegen/llvm-22.1.4/include/llvm/IR/Attributes.td");

/// Spellings llvmkit's lexer accepts that LLVM 22.1.4 does not. Every entry is
/// a deliberate extension with a reason, not an oversight; anything else
/// appearing here is an invention and must be removed or argued for in
/// `docs/divergences.md`.
///
/// * `exnref` — LLVM 22.1.4 declares `exnref` as a WebAssembly `ValueType`
///   (`include/llvm/CodeGen/ValueTypes.td`) and uses it for the
///   `int_wasm_ref_null_exn` family (`IntrinsicsWebAssembly.td`), but gives it
///   no `Type::TypeID`, no `TYPEKEYWORD`, and no `IIT_VT<exnref>` — so
///   `llvm_exnref_ty`'s `Sig` filters to the empty list and the type is
///   unwritable in `.ll`. llvmkit's TableGen port models it (`TypeData::
///   WasmExnRef`, `CUSTOM_IIT_WASM_EXNREF`) so the WebAssembly intrinsic
///   signatures are well-formed, and `Display for TypeData` prints `exnref`.
///   The keyword exists so that printed output re-parses; dropping it alone
///   would open a print-but-not-parse hole. Recorded as divergence 103.
const NON_UPSTREAM_KEYWORDS: &[&str] = &["exnref"];

// ── Readers over the vendored C++ ────────────────────────────────────────────

/// Every `NAME(...)` invocation in `src`, as the raw paren-balanced argument
/// text.
///
/// The preceding-byte guard is what keeps `KEYWORD(` from also matching inside
/// `TYPEKEYWORD(`, `INSTKEYWORD(` and `DBGRECORDTYPEKEYWORD(`; the balance
/// counter is what keeps `TYPEKEYWORD("void", Type::getVoidTy(Context))` from
/// being cut at its inner `)`.
fn macro_invocations(src: &str, name: &str) -> Vec<String> {
    let bytes = src.as_bytes();
    let needle = format!("{name}(");
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = src[from..].find(&needle) {
        let open = from + offset;
        from = open + needle.len();
        if open > 0 {
            let previous = bytes[open - 1];
            if previous.is_ascii_alphanumeric() || previous == b'_' {
                continue;
            }
        }
        let mut depth = 1usize;
        let mut index = from;
        while index < bytes.len() && depth > 0 {
            match bytes[index] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            index += 1;
        }
        if depth == 0 {
            out.push(src[from..index - 1].to_string());
        }
    }
    out
}

/// The `#define NAME(params)` … `#undef NAME` region a macro table lives in.
/// Reading the whole file instead would sweep up the macro's own definition
/// and any later mention. `signature` carries the parameter list so that
/// `#define KEYWORD(STR)` is not also found inside `#define TYPEKEYWORD(...)`.
fn macro_region<'a>(src: &'a str, name: &str, signature: &str) -> &'a str {
    let open = format!("#define {signature}");
    let close = format!("#undef {name}");
    let start = src
        .find(open.as_str())
        .unwrap_or_else(|| panic!("vendored LLLexer.cpp has no `{open}`"));
    let end = src[start..]
        .find(close.as_str())
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("vendored LLLexer.cpp has no `{close}`"));
    &src[start..end]
}

/// Every double-quoted literal in `text`, in order.
fn string_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        out.push(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    out
}

/// `KEYWORD(x)` — the plain-keyword table, minus the two macro parameters that
/// share its spelling (`STR` from `#define KEYWORD(STR)`, `DISPLAY_NAME` from
/// the `ATTRIBUTE_ENUM` bridge to `Attributes.inc`).
fn upstream_keywords(src: &str) -> BTreeSet<String> {
    macro_invocations(macro_region(src, "KEYWORD", "KEYWORD(STR)"), "KEYWORD")
        .into_iter()
        .map(|argument| argument.trim().to_string())
        .filter(|word| word != "STR" && word != "DISPLAY_NAME")
        .collect()
}

/// `TYPEKEYWORD("x", …)` — the argument is a string literal, unlike every
/// other table here.
fn upstream_type_keywords(src: &str) -> BTreeSet<String> {
    macro_invocations(
        macro_region(src, "TYPEKEYWORD", "TYPEKEYWORD(STR, LLVMTY)"),
        "TYPEKEYWORD",
    )
    .into_iter()
    .filter_map(|argument| string_literals(&argument).into_iter().next())
    .collect()
}

/// `INSTKEYWORD(x, Enum)` — the keyword is the first argument, stringified by
/// the macro as `#STR`.
fn upstream_instruction_keywords(src: &str) -> BTreeSet<String> {
    macro_invocations(
        macro_region(src, "INSTKEYWORD", "INSTKEYWORD(STR, Enum)"),
        "INSTKEYWORD",
    )
    .into_iter()
    .filter_map(|argument| Some(argument.split(',').next()?.trim().to_string()))
    .filter(|word| word != "STR")
    .collect()
}

/// `DWKEYWORD(TYPE, TOKEN)` — `(infix, lltok kind)`, e.g. `("TAG", "DwarfTag")`.
/// The macro matches `DW_<TYPE>_`, so the infix is not itself a spelling.
fn upstream_dwarf_families(src: &str) -> Vec<(String, String)> {
    macro_invocations(
        macro_region(src, "DWKEYWORD", "DWKEYWORD(TYPE, TOKEN)"),
        "DWKEYWORD",
    )
    .into_iter()
    .filter_map(|argument| {
        let (infix, token) = argument.split_once(',')?;
        Some((infix.trim().to_string(), token.trim().to_string()))
    })
    .filter(|(infix, _)| infix != "TYPE")
    .collect()
}

/// `DBGRECORDTYPEKEYWORD(x)` — matches `dbg_<x>` and carries `<x>` as the
/// payload, which is why llvmkit's `Token::DbgRecordType` holds the suffix.
fn upstream_debug_record_types(src: &str) -> BTreeSet<String> {
    macro_invocations(
        macro_region(src, "DBGRECORDTYPEKEYWORD", "DBGRECORDTYPEKEYWORD(STR)"),
        "DBGRECORDTYPEKEYWORD",
    )
    .into_iter()
    .map(|argument| argument.trim().to_string())
    .filter(|word| word != "STR")
    .collect()
}

/// The literals in the `if (…)` guarding `return lltok::<kind>;`.
///
/// Serves both tail shapes: a prefix family (`Keyword.starts_with("DIFlag")`)
/// yields one literal, an exact-word family
/// (`Keyword == "GNU" || Keyword == "Apple" || …`) yields all of them.
fn upstream_tail_family(src: &str, kind: &str) -> Vec<String> {
    let needle = format!("return lltok::{kind};");
    let end = src
        .find(needle.as_str())
        .unwrap_or_else(|| panic!("vendored LLLexer.cpp has no `{needle}`"));
    let start = src[..end]
        .rfind("if (")
        .unwrap_or_else(|| panic!("no `if (` guards `{needle}`"));
    string_literals(&src[start..end])
}

/// The single-character `case 'X': return lltok::Y;` arms of
/// `LLLexer::LexToken` — the punctuation that carries no payload.
///
/// Every other arm is skipped by construction, because the filter is that the
/// arm body be *exactly* one `return lltok::…;`: the whitespace fallthroughs
/// and the two `continue`s have no return, the `'@'` / `'%'` / `'!'` / `'#'`
/// / `'^'` / `'$'` / `'"'` / `'+'` arms delegate to a `Lex*` routine, the digit
/// run falls through to `LexDigitOrNegative`, and `'.'` and `'/'` branch.
fn upstream_punctuation(src: &str) -> Vec<(char, String)> {
    let start = src
        .find("lltok::Kind LLLexer::LexToken()")
        .expect("vendored LLLexer.cpp has no LLLexer::LexToken");
    let body = &src[start..];
    let body = &body[..body.find("\n}").expect("end of LLLexer::LexToken")];

    let mut out = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("case '") {
        rest = &rest[open + "case '".len()..];
        let Some(literal) = rest.chars().next() else {
            break;
        };
        let Some(arm) = rest[literal.len_utf8()..].strip_prefix("':") else {
            continue;
        };
        let arm = match arm.find("case ") {
            Some(next) => &arm[..next],
            None => arm,
        };
        let collapsed = arm.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(kind) = collapsed
            .strip_prefix("return lltok::")
            .and_then(|kind| kind.strip_suffix(';'))
        {
            out.push((literal, kind.to_string()));
        }
    }
    out
}

/// The enum-valued attribute keywords `Attributes.inc` contributes to the
/// `KEYWORD` block, i.e. the classes `Attributes::emitTargetIndependentNames`
/// feeds to `ATTRIBUTE_ENUM` — the only one of its three macros `LLLexer.cpp`
/// defines. `StrBoolAttr` and `ComplexStrAttr` fall through to the empty
/// `ATTRIBUTE_ALL` and are spelled as quoted strings instead.
fn upstream_attribute_keywords(src: &str) -> BTreeSet<String> {
    const ENUM_VALUED: [&str; 5] = [
        "EnumAttr",
        "TypeAttr",
        "IntAttr",
        "ConstantRangeAttr",
        "ConstantRangeListAttr",
    ];
    let mut out = BTreeSet::new();
    // Upstream wraps a long `def` across lines, so defs are joined on `;`
    // first — the same reader shape `attribute_td_drift.rs` uses, and for the
    // same reason.
    let mut current = String::new();
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("def ") {
            current = line.to_owned();
        } else if !current.is_empty() {
            current.push(' ');
            current.push_str(line);
        }
        if !current.ends_with(';') {
            continue;
        }
        let definition = std::mem::take(&mut current);
        let Some((name, declaration)) = definition[4..].split_once(':') else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let Some((class, arguments)) = declaration.trim().split_once('<') else {
            continue;
        };
        if !ENUM_VALUED.contains(&class.trim()) {
            continue;
        }
        if let Some(keyword) = string_literals(arguments).into_iter().next()
            && !keyword.is_empty()
        {
            out.insert(keyword);
        }
    }
    out
}

/// Every spelling `LLLexer::LexIdentifier` resolves by exact match: its
/// `KEYWORD` block (with the `Attributes.inc` names spliced in), its
/// `TYPEKEYWORD` table and its `INSTKEYWORD` table. The prefix and tail
/// families are not exact matches and are checked family by family instead.
fn upstream_all_spellings() -> BTreeSet<String> {
    let mut all = upstream_keywords(LLLEXER_CPP);
    all.extend(upstream_attribute_keywords(ATTRIBUTES_TD));
    all.extend(upstream_type_keywords(LLLEXER_CPP));
    all.extend(upstream_instruction_keywords(LLLEXER_CPP));
    all
}

// ── Reader over llvmkit's own table ──────────────────────────────────────────

/// Every spelling `ll_lexer::keywords::classify_word` matches, read out of its
/// source.
///
/// Source-scraping is the only mechanism available for the *reverse*
/// direction: `classify_word` is `pub(super)`, `Keyword` has no `ALL` and no
/// spelling accessor, and a word set cannot be enumerated by probing. The
/// forward direction below never needs this — it drives the public `Lexer`.
/// `llvmkit_keyword_table_is_readable` is the guard that a reformat fails
/// loudly here instead of quietly matching nothing.
fn llvmkit_spellings() -> BTreeSet<String> {
    const SOURCE: &str = include_str!("../src/ll_lexer/keywords.rs");
    let production = SOURCE
        .split_once("#[cfg(test)]")
        .map(|(before, _)| before)
        .unwrap_or(SOURCE);
    production
        .lines()
        .filter_map(|line| {
            let (literal, _) = line.trim().strip_prefix("b\"")?.split_once("\" =>")?;
            Some(literal.to_string())
        })
        .collect()
}

// ── Guards on this file's own readers ────────────────────────────────────────

/// The vendored sources are readable and hold the tables in the shape the
/// readers above assume. A change in `LLLexer.cpp`'s macro layout fails here
/// rather than showing up as a mysteriously shrinking keyword set — which is
/// the failure mode `attribute_td_drift.rs` hit three separate times.
#[test]
fn vendored_lllexer_cpp_is_parseable() {
    let keywords = upstream_keywords(LLLEXER_CPP);
    assert_eq!(
        keywords.len(),
        329,
        "expected the LLVM 22.1.4 explicit KEYWORD block"
    );
    assert!(keywords.contains("zeroinitializer"));
    assert!(keywords.contains("notcold"));
    // `c` and `cc` are one and two bytes long; a reader that trims wrongly
    // loses exactly these.
    assert!(keywords.contains("c") && keywords.contains("cc") && keywords.contains("x"));
    // `KEYWORD(DISPLAY_NAME)` is the `ATTRIBUTE_ENUM` bridge, not a spelling.
    assert!(!keywords.contains("DISPLAY_NAME") && !keywords.contains("STR"));

    assert_eq!(
        upstream_type_keywords(LLLEXER_CPP).len(),
        13,
        "expected the LLVM 22.1.4 TYPEKEYWORD table"
    );
    assert_eq!(
        upstream_instruction_keywords(LLLEXER_CPP).len(),
        66,
        "expected the LLVM 22.1.4 INSTKEYWORD table"
    );
    assert_eq!(upstream_dwarf_families(LLLEXER_CPP).len(), 9);
    assert_eq!(upstream_debug_record_types(LLLEXER_CPP).len(), 5);
    assert_eq!(
        upstream_attribute_keywords(ATTRIBUTES_TD).len(),
        101,
        "expected the LLVM 22.1.4 ATTRIBUTE_ENUM set"
    );
}

/// The reverse-direction reader sees llvmkit's whole table. A `keywords.rs`
/// reformat that breaks the scrape must fail here, not silently reduce the
/// backward check below to a tautology.
#[test]
fn llvmkit_keyword_table_is_readable() {
    let mine = llvmkit_spellings();
    assert_eq!(
        mine.len(),
        510,
        "the `classify_word` scrape found the wrong number of arms — if the \
         table was reformatted, fix `llvmkit_spellings`, do not adjust this \
         number to match"
    );
    assert!(mine.contains("getelementptr") && mine.contains("x") && mine.contains("notcold"));
}

// ── Forward: every upstream spelling, in the right family ────────────────────

/// Lex `word` on its own and return the first token.
fn lex_one(word: &str) -> Token<'_> {
    Lexer::from(word)
        .next_token()
        .unwrap_or_else(|error| panic!("lexing {word:?} failed: {error}"))
        .value
}

/// Every `KEYWORD(...)` spelling — the explicit block plus the attribute
/// keywords `Attributes.inc` splices into it — lexes to a plain keyword token.
///
/// Family, not identity: `Keyword` has no spelling accessor, so what is pinned
/// is that the word is *a* keyword rather than a type, an opcode, or an error.
/// Which keyword it is falls out of the parser tests.
#[test]
fn every_upstream_keyword_lexes_as_a_keyword() {
    let mut wrong = Vec::new();
    let mut expected = upstream_keywords(LLLEXER_CPP);
    expected.extend(upstream_attribute_keywords(ATTRIBUTES_TD));
    for word in &expected {
        if !matches!(lex_one(word), Token::Kw(_)) {
            wrong.push(format!("{word} lexes as {:?}", lex_one(word)));
        }
    }
    assert!(
        wrong.is_empty(),
        "these `LLLexer` KEYWORD spellings are not plain keywords here:\n  {}",
        wrong.join("\n  ")
    );
}

/// Every `TYPEKEYWORD("...", ...)` spelling lexes to a primitive type.
#[test]
fn every_upstream_type_keyword_lexes_as_a_type() {
    let mut wrong = Vec::new();
    for word in upstream_type_keywords(LLLEXER_CPP) {
        if !matches!(lex_one(&word), Token::PrimitiveType(_)) {
            wrong.push(format!("{word} lexes as {:?}", lex_one(&word)));
        }
    }
    assert!(
        wrong.is_empty(),
        "these `LLLexer` TYPEKEYWORD spellings are not primitive types here:\n  {}",
        wrong.join("\n  ")
    );
}

/// Every `INSTKEYWORD(..., ...)` spelling lexes to an instruction opcode.
///
/// This is the table the program's own inventory flagged as never mechanically
/// diffed. It agrees today; the point is that it cannot quietly stop agreeing.
#[test]
fn every_upstream_instruction_keyword_lexes_as_an_opcode() {
    let mut wrong = Vec::new();
    for word in upstream_instruction_keywords(LLLEXER_CPP) {
        if !matches!(lex_one(&word), Token::Instruction(_)) {
            wrong.push(format!("{word} lexes as {:?}", lex_one(&word)));
        }
    }
    assert!(
        wrong.is_empty(),
        "these `LLLexer` INSTKEYWORD spellings are not opcodes here:\n  {}",
        wrong.join("\n  ")
    );
}

/// Every payload-free punctuation character `LLLexer::LexToken` returns
/// directly lexes to its own llvmkit token.
///
/// The keyword tables get all the attention, but the punctuation set is a
/// table too, and a `<` read as a `>` would be just as silent. Upstream's arms
/// are one line each, so the reader can insist the whole arm body is a single
/// `return`; anything else is a delegating or branching arm and is skipped.
#[test]
fn every_upstream_punctuation_character_lexes_to_its_own_token() {
    let arms = upstream_punctuation(LLLEXER_CPP);
    assert_eq!(
        arms.len(),
        13,
        "expected the LLVM 22.1.4 payload-free punctuation arms, got {arms:?}"
    );
    let mut wrong = Vec::new();
    for (character, kind) in arms {
        let text = character.to_string();
        let token = lex_one(&text);
        let expected = match kind.as_str() {
            "colon" => Token::Colon,
            "equal" => Token::Equal,
            "lsquare" => Token::LSquare,
            "rsquare" => Token::RSquare,
            "lbrace" => Token::LBrace,
            "rbrace" => Token::RBrace,
            "less" => Token::Less,
            "greater" => Token::Greater,
            "lparen" => Token::LParen,
            "rparen" => Token::RParen,
            "comma" => Token::Comma,
            "star" => Token::Star,
            "bar" => Token::Bar,
            other => panic!("new `lltok::{other}` punctuation arm — give it a llvmkit token"),
        };
        if token != expected {
            wrong.push(format!(
                "{character:?} (lltok::{kind}) lexes as {token:?}, expected {expected:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "`LexToken`'s punctuation drifted:\n  {}",
        wrong.join("\n  ")
    );
}

/// Every `DWKEYWORD(TYPE, TOKEN)` family maps `DW_<TYPE>_…` to its own token.
///
/// The vocabularies behind these are `dwarf_def_drift.rs`'s business; what is
/// checked here is that the nine *families* exist and route to nine distinct
/// tokens, which is what `LLLexer` guarantees by construction and llvmkit
/// spells out by hand.
#[test]
fn every_upstream_dwarf_family_lexes_to_its_own_token() {
    let mut wrong = Vec::new();
    for (infix, kind) in upstream_dwarf_families(LLLEXER_CPP) {
        let word = format!("DW_{infix}_probe");
        let token = lex_one(&word);
        let matched = match kind.as_str() {
            "DwarfTag" => matches!(token, Token::DwarfTag(_)),
            "DwarfAttEncoding" => matches!(token, Token::DwarfAttEncoding(_)),
            "DwarfVirtuality" => matches!(token, Token::DwarfVirtuality(_)),
            "DwarfLang" => matches!(token, Token::DwarfLang(_)),
            "DwarfSourceLangName" => matches!(token, Token::DwarfSourceLangName(_)),
            "DwarfCC" => matches!(token, Token::DwarfCc(_)),
            "DwarfOp" => matches!(token, Token::DwarfOp(_)),
            "DwarfMacinfo" => matches!(token, Token::DwarfMacinfo(_)),
            "DwarfEnumKind" => matches!(token, Token::DwarfEnumKind(_)),
            other => panic!("new DWKEYWORD family `{other}` — give it a llvmkit token"),
        };
        if !matched {
            wrong.push(format!("{word} (lltok::{kind}) lexes as {token:?}"));
        }
        // The payload is the whole keyword, per `StrVal.assign(Keyword.begin(),
        // Keyword.end())` — not the suffix.
        let payload = match token {
            Token::DwarfTag(s)
            | Token::DwarfAttEncoding(s)
            | Token::DwarfVirtuality(s)
            | Token::DwarfLang(s)
            | Token::DwarfSourceLangName(s)
            | Token::DwarfCc(s)
            | Token::DwarfOp(s)
            | Token::DwarfMacinfo(s)
            | Token::DwarfEnumKind(s) => Some(s),
            _ => None,
        };
        if let Some(payload) = payload
            && payload != word
        {
            wrong.push(format!("{word} carries {payload:?}, not the whole keyword"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the DWKEYWORD families drifted:\n  {}",
        wrong.join("\n  ")
    );
}

/// Every `DBGRECORDTYPEKEYWORD(x)` matches `dbg_<x>` and carries `<x>`.
#[test]
fn every_upstream_debug_record_type_lexes_with_its_suffix() {
    let mut wrong = Vec::new();
    for suffix in upstream_debug_record_types(LLLEXER_CPP) {
        let word = format!("dbg_{suffix}");
        match lex_one(&word) {
            Token::DbgRecordType(payload) if payload == suffix => {}
            other => wrong.push(format!("{word} lexes as {other:?}")),
        }
    }
    assert!(
        wrong.is_empty(),
        "the DBGRECORDTYPEKEYWORD table drifted:\n  {}",
        wrong.join("\n  ")
    );
}

/// "Is this the token family the `lltok` kind names?" — one case of a tail
/// family, paired with the kind it is checking.
type TokenPredicate = fn(&Token<'_>) -> bool;

/// The three prefix families in `LexIdentifier`'s tail — `DIFlag`, `DISPFlag`,
/// `CSK_` — still match by prefix, and still route to their own tokens.
#[test]
fn the_tail_prefix_families_still_match_by_prefix() {
    let cases: [(&str, TokenPredicate); 3] = [
        ("DIFlag", |t| matches!(t, Token::DiFlag(_))),
        ("DISPFlag", |t| matches!(t, Token::DiSpFlag(_))),
        ("ChecksumKind", |t| matches!(t, Token::ChecksumKind(_))),
    ];
    for (kind, is_expected) in cases {
        let literals = upstream_tail_family(LLLEXER_CPP, kind);
        assert_eq!(
            literals.len(),
            1,
            "lltok::{kind} is guarded by {literals:?}, not one prefix"
        );
        let word = format!("{}Probe", literals[0]);
        let token = lex_one(&word);
        assert!(
            is_expected(&token),
            "{word} should be a lltok::{kind}, lexed as {token:?}"
        );
    }
}

/// The three exact-word families in `LexIdentifier`'s tail — emission kind,
/// name-table kind, fixed-point kind — accept exactly upstream's words.
///
/// These are the only word lists in the lexer with no `.def` or `.td` behind
/// them, so before this test nothing at all tied them to upstream. Both
/// directions are checked: every upstream word lexes to the family, and the
/// count matches so a llvmkit-only extra word would have to show up in
/// `LLLexer.cpp` too.
#[test]
fn the_tail_exact_word_families_accept_exactly_upstreams_words() {
    let cases: [(&str, TokenPredicate); 3] = [
        ("EmissionKind", |t| matches!(t, Token::EmissionKind(_))),
        ("NameTableKind", |t| matches!(t, Token::NameTableKind(_))),
        ("FixedPointKind", |t| matches!(t, Token::FixedPointKind(_))),
    ];
    let mut wrong = Vec::new();
    for (kind, is_expected) in cases {
        let words = upstream_tail_family(LLLEXER_CPP, kind);
        assert!(
            words.len() >= 3,
            "lltok::{kind} is guarded by {words:?} — the reader lost its `||` chain"
        );
        for word in &words {
            let token = lex_one(word);
            if !is_expected(&token) {
                wrong.push(format!("{word} (lltok::{kind}) lexes as {token:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the exact-word tail families drifted:\n  {}",
        wrong.join("\n  ")
    );
    // The counts upstream writes today, so an added word fails above rather
    // than passing unnoticed.
    assert_eq!(upstream_tail_family(LLLEXER_CPP, "EmissionKind").len(), 4);
    assert_eq!(upstream_tail_family(LLLEXER_CPP, "NameTableKind").len(), 4);
    assert_eq!(upstream_tail_family(LLLEXER_CPP, "FixedPointKind").len(), 3);
}

// ── Backward: nothing llvmkit-only ───────────────────────────────────────────

/// Every spelling llvmkit's keyword table matches is one `LLLexer` matches,
/// except the entries [`NON_UPSTREAM_KEYWORDS`] argues for.
///
/// This is the direction that fails silently: llvmkit accepting a word LLVM
/// rejects produces a module that `llvm-as` cannot read back, with no test
/// anywhere to notice.
#[test]
fn llvmkit_knows_no_keyword_upstream_dropped() {
    let allowed: BTreeSet<&str> = NON_UPSTREAM_KEYWORDS.iter().copied().collect();
    let upstream = upstream_all_spellings();
    let extra: Vec<String> = llvmkit_spellings()
        .into_iter()
        .filter(|word| !upstream.contains(word) && !allowed.contains(word.as_str()))
        .collect();
    assert!(
        extra.is_empty(),
        "these spellings are llvmkit inventions — remove them, or add them to \
         NON_UPSTREAM_KEYWORDS with the reason:\n  {}",
        extra.join("\n  ")
    );
}

/// …and nothing upstream matches is missing here. The forward tests above
/// already prove each family, but they check family membership one word at a
/// time; this states the set relation, so a spelling that stops being matched
/// at all is named directly.
#[test]
fn llvmkit_knows_every_keyword_upstream_has() {
    let mine = llvmkit_spellings();
    let missing: Vec<String> = upstream_all_spellings()
        .into_iter()
        .filter(|word| !mine.contains(word))
        .collect();
    assert!(
        missing.is_empty(),
        "these LLVM 22.1.4 spellings are not in `classify_word`:\n  {}",
        missing.join("\n  ")
    );
}

/// `NON_UPSTREAM_KEYWORDS` has no stale entry — an extension upstream later
/// adopts stops being an extension.
#[test]
fn the_extension_list_has_no_stale_entries() {
    let upstream = upstream_all_spellings();
    let adopted: Vec<&&str> = NON_UPSTREAM_KEYWORDS
        .iter()
        .filter(|word| upstream.contains(**word))
        .collect();
    assert!(
        adopted.is_empty(),
        "LLVM 22.1.4 now defines these, so they are no longer llvmkit \
         extensions: {adopted:?}"
    );
}

// ── Order, and the two tails after the tables ────────────────────────────────

/// llvmkit checks the prefixed families *before* the keyword table;
/// `LexIdentifier` checks them after. The orders are observationally identical
/// only while the two sets stay disjoint, and this is what says so.
///
/// A future upstream keyword called `DIFlagged` or `dbg_v` would be read as a
/// `DIFlag` / `DbgRecordType` here and as a keyword there, and nothing else in
/// the suite would notice.
#[test]
fn no_upstream_keyword_is_also_claimed_by_a_prefixed_family() {
    let mut prefixes: Vec<String> = upstream_dwarf_families(LLLEXER_CPP)
        .into_iter()
        .map(|(infix, _)| format!("DW_{infix}_"))
        .collect();
    prefixes.push("dbg_".to_string());
    for kind in ["DIFlag", "DISPFlag", "ChecksumKind"] {
        prefixes.extend(upstream_tail_family(LLLEXER_CPP, kind));
    }
    let mut exact: BTreeSet<String> = BTreeSet::new();
    for kind in ["EmissionKind", "NameTableKind", "FixedPointKind"] {
        exact.extend(upstream_tail_family(LLLEXER_CPP, kind));
    }

    let mut collisions = Vec::new();
    for word in upstream_all_spellings() {
        if let Some(prefix) = prefixes.iter().find(|p| word.starts_with(p.as_str())) {
            collisions.push(format!("{word} also matches the `{prefix}` family"));
        }
        if exact.contains(&word) {
            collisions.push(format!("{word} is also a tail exact-word"));
        }
    }
    assert!(
        collisions.is_empty(),
        "llvmkit's classification order is no longer equivalent to \
         `LexIdentifier`'s — move the keyword table ahead of \
         `classify_prefixed`:\n  {}",
        collisions.join("\n  ")
    );
}

/// `// If this is "cc1234", return this as just "cc".` — the rewind fires on
/// the two source bytes, so *any* otherwise-unknown word opening `cc` becomes
/// `kw_cc` plus whatever follows. `cc` and `ccc` never reach it.
#[test]
fn an_unknown_word_opening_cc_rewinds_to_kw_cc() {
    let mut lexer = Lexer::from("ccfoo");
    let first = lexer.next_token().expect("kw_cc").value;
    assert!(matches!(first, Token::Kw(_)), "{first:?}");
    // The cursor stopped at `TokStart+2`, so `foo` is still to come.
    let second = lexer.next_token().expect("the rewound tail").value;
    assert_eq!(second, Token::Error, "`foo` is no keyword: {second:?}");

    let mut lexer = Lexer::from("cc1234");
    assert!(matches!(
        lexer.next_token().expect("kw_cc").value,
        Token::Kw(_)
    ));
    assert!(matches!(
        lexer.next_token().expect("the rewound digits").value,
        Token::IntegerLit(_)
    ));
}

/// `// Finally, if this isn't known, return an error.` A word no family claims
/// is `lltok::Error`, not a keyword invented on the spot.
#[test]
fn a_word_no_family_claims_is_an_error_token() {
    assert_eq!(lex_one("definitely_not_a_keyword"), Token::Error);
    // Bare specialized-metadata names are the case that regressed: llvmkit
    // carried a `Token::SpecializedMetadata` family for eighteen of the
    // thirty-two `DI*` node names, which `LLLexer` has no counterpart for —
    // `!DIFile` is a `lltok::MetadataVar` out of `LLLexer::LexExclaim`, and
    // bare `DIFile` is an error.
    assert_eq!(lex_one("DIFile"), Token::Error);
    assert_eq!(lex_one("DIExpression"), Token::Error);
    match lex_one("!DIFile") {
        Token::MetadataVar(name) => assert_eq!(name.as_ref(), b"DIFile"),
        other => panic!("`!DIFile` should be a MetadataVar, got {other:?}"),
    }
}

// ── The token enum itself ────────────────────────────────────────────────────

/// Every `lltok::Kind` that is not a `kw_*`, paired with the llvmkit
/// [`Token`] variant carrying it. The `kw_*` half is
/// [`Token::Kw`] plus [`Token::Instruction`] — llvmkit splits the opcode
/// keywords into their own variant, which is a narrowing of upstream's space,
/// not an addition to it.
const LLTOK_KINDS: &[(&str, &str)] = &[
    ("Eof", "Token::Eof"),
    ("Error", "Token::Error"),
    ("dotdotdot", "Token::DotDotDot"),
    ("equal", "Token::Equal"),
    ("comma", "Token::Comma"),
    ("star", "Token::Star"),
    ("lsquare", "Token::LSquare"),
    ("rsquare", "Token::RSquare"),
    ("lbrace", "Token::LBrace"),
    ("rbrace", "Token::RBrace"),
    ("less", "Token::Less"),
    ("greater", "Token::Greater"),
    ("lparen", "Token::LParen"),
    ("rparen", "Token::RParen"),
    ("exclaim", "Token::Exclaim"),
    ("bar", "Token::Bar"),
    ("colon", "Token::Colon"),
    ("hash", "Token::Hash"),
    ("LabelID", "Token::LabelId"),
    ("GlobalID", "Token::GlobalId"),
    ("LocalVarID", "Token::LocalVarId"),
    ("AttrGrpID", "Token::AttrGrpId"),
    ("SummaryID", "Token::SummaryId"),
    ("LabelStr", "Token::LabelStr"),
    ("GlobalVar", "Token::GlobalVar"),
    ("ComdatVar", "Token::ComdatVar"),
    ("LocalVar", "Token::LocalVar"),
    ("MetadataVar", "Token::MetadataVar"),
    ("StringConstant", "Token::StringConstant"),
    ("DwarfTag", "Token::DwarfTag"),
    ("DwarfAttEncoding", "Token::DwarfAttEncoding"),
    ("DwarfVirtuality", "Token::DwarfVirtuality"),
    ("DwarfLang", "Token::DwarfLang"),
    ("DwarfSourceLangName", "Token::DwarfSourceLangName"),
    ("DwarfCC", "Token::DwarfCc"),
    ("EmissionKind", "Token::EmissionKind"),
    ("NameTableKind", "Token::NameTableKind"),
    ("FixedPointKind", "Token::FixedPointKind"),
    ("DwarfOp", "Token::DwarfOp"),
    ("DIFlag", "Token::DiFlag"),
    ("DISPFlag", "Token::DiSpFlag"),
    ("DwarfMacinfo", "Token::DwarfMacinfo"),
    ("ChecksumKind", "Token::ChecksumKind"),
    ("DbgRecordType", "Token::DbgRecordType"),
    ("DwarfEnumKind", "Token::DwarfEnumKind"),
    ("Type", "Token::PrimitiveType"),
    ("APFloat", "Token::FloatLit"),
    ("APSInt", "Token::IntegerLit"),
];

/// Every enumerator literally written in `lltok::Kind` — the `kw_##` line from
/// the `ATTRIBUTE_ENUM` bridge is skipped, since it names no single token.
fn lltok_kinds(src: &str) -> Vec<String> {
    let body = &src[src.find("enum Kind {").expect("enum Kind") + "enum Kind {".len()..];
    let body = &body[..body.find("\n};").expect("end of enum Kind")];
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for part in line.split(',') {
            let part = part.trim();
            if part.is_empty() || part.contains("##") {
                continue;
            }
            if part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                out.push(part.to_string());
            }
        }
    }
    out
}

/// Every non-`kw_` `lltok::Kind` has a llvmkit [`Token`] variant, and
/// [`LLTOK_KINDS`] names no kind upstream dropped.
///
/// This is the check that would have caught `lltok::LabelID`: llvmkit had no
/// numeric-label token at all until W14b, so `42:` and `"42":` were the same
/// `LabelStr` and a label too large for an `unsigned` became a block *named*
/// `4294967296` instead of `invalid value number (too large)`.
#[test]
fn every_lltok_kind_has_a_llvmkit_token() {
    let declared: Vec<String> = lltok_kinds(LLTOKEN_H)
        .into_iter()
        .filter(|kind| !kind.starts_with("kw_"))
        .collect();
    let mapped: BTreeSet<&str> = LLTOK_KINDS.iter().map(|(kind, _)| *kind).collect();

    let unmapped: Vec<&String> = declared
        .iter()
        .filter(|kind| !mapped.contains(kind.as_str()))
        .collect();
    assert!(
        unmapped.is_empty(),
        "LLVM 22.1.4 declares these `lltok::Kind`s with no llvmkit `Token` \
         variant: {unmapped:?}"
    );

    let declared_set: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    let stale: Vec<&str> = LLTOK_KINDS
        .iter()
        .map(|(kind, _)| *kind)
        .filter(|kind| !declared_set.contains(kind))
        .collect();
    assert!(
        stale.is_empty(),
        "LLTOK_KINDS names kinds LLVM 22.1.4 does not declare: {stale:?}"
    );
    assert_eq!(declared.len(), LLTOK_KINDS.len());
}

/// The `kw_*` half of `lltok::Kind` is exactly what `LLLexer` lexes as a
/// keyword or an opcode — no dead token, no unlisted one.
///
/// Upstream cannot drift here (both files `#include` the same generated
/// `Attributes.inc`), which is precisely why it makes a good anchor: any
/// mismatch means this file's readers are wrong, not that LLVM is.
#[test]
fn the_lltok_keyword_space_matches_what_the_lexer_lexes() {
    let declared: BTreeSet<String> = lltok_kinds(LLTOKEN_H)
        .into_iter()
        .filter_map(|kind| kind.strip_prefix("kw_").map(str::to_string))
        .collect();
    let mut lexed = upstream_keywords(LLLEXER_CPP);
    lexed.extend(upstream_instruction_keywords(LLLEXER_CPP));
    assert_eq!(
        declared, lexed,
        "the `kw_*` enumerators and the KEYWORD/INSTKEYWORD tables disagree — \
         this file's readers are misreading one of them"
    );
}
