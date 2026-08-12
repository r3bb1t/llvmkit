//! Anti-drift guard for the attribute keyword table.
//!
//! `ll_lexer/keywords.rs` hand-mirrors LLVM's `Attributes.td`, and that is how
//! Milestone 0's ~21 missing keywords went unnoticed: nothing tied the table to
//! its source. Upstream does not have this problem because its lexer and parser
//! both `#include` the TableGen-generated `Attributes.inc`
//! (`LLLexer.cpp:701-704`, `LLParser.cpp:1547-1551`), so their list *cannot*
//! drift.
//!
//! Full generation is the wrong shape here: llvmkit deliberately models a
//! subset of LLVM's attributes, and generating the table would force modeling
//! all of them (and would mean generating part of the 700-variant `Keyword`
//! enum). This test gives the same guarantee without that cost — it parses the
//! vendored `Attributes.td` and asserts every attribute is either **accepted**
//! by the parser in a position `Attributes.td` declares for it, or **listed
//! below** as deliberately not modeled yet.
//!
//! So a new upstream attribute, or one we silently stop accepting, fails CI.
//! The `.td` is vendored under this crate's `tablegen/` (tracked, unlike
//! `orig_cpp/`), so the guard runs everywhere the tests do.

use std::collections::BTreeSet;

use llvmkit_asmparser::parse_dynamic;
use llvmkit_ir::StrBoolAttrKind;

const ATTRIBUTES_TD: &str = include_str!("../tablegen/llvm-22.1.4/include/llvm/IR/Attributes.td");

/// Attributes LLVM 22.1.4 defines that llvmkit does not model yet. Every entry
/// is a deliberate omission, not an oversight: adding one to the parser means
/// deleting its line here, and a new upstream attribute fails this test until
/// it is either implemented or consciously added.
///
/// Kept as the spelled `.ll` keyword, sorted. Each of the four needs a
/// **grammar**, not just a keyword — every one takes an argument the parser
/// has no production for yet, which is why they outlived the sweep that wired
/// the other thirty-nine.
const NOT_YET_MODELED: &[&str] = &["allocsize", "initializes", "preallocated", "vscale_range"];

/// One attribute as `Attributes.td` declares it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TdAttribute {
    keyword: String,
    /// `EnumAttr`, `IntAttr`, `TypeAttr`, … — decides the spelling we probe.
    kind: String,
    fn_attr: bool,
    param_attr: bool,
    ret_attr: bool,
}

/// Each `def … ;` in the file, joined into one logical line.
///
/// Upstream wraps a `def` across lines whenever the declaration is long, and
/// reading this file line by line silently mis-reads exactly those. It did:
/// `dereferenceable_or_null` and `speculative_load_hardening` came back
/// declaring *no* position, which made the probe below vacuous for both, and
/// `nocreateundeforpoison` was not seen at all — so a new upstream attribute
/// would slip past this guard purely by being declared on two lines.
///
/// No string literal in `Attributes.td` contains a `;`, so terminating a def
/// on one is safe.
fn attribute_defs(src: &str) -> Vec<String> {
    let mut defs = Vec::new();
    let mut current = String::new();
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("def ") {
            current = line.to_owned();
        } else if !current.is_empty() {
            current.push(' ');
            current.push_str(line);
        }
        if current.ends_with(';') {
            defs.push(std::mem::take(&mut current));
        }
    }
    defs
}

/// Parse the `def <Name> : <Kind>Attr<"<keyword>", …, [<positions>]>;` defs.
/// `StrBoolAttr` carries no positions (`Attr<S, IntersectPreserve, []>`), so
/// the position-probing tests below cannot exercise it and it is skipped
/// here; `str_bool_attributes_have_typed_readers` covers those declarations
/// against the `StrBoolAttrKind` reader enum instead.
fn parse_attributes_td(src: &str) -> Vec<TdAttribute> {
    let mut out = Vec::new();
    for def in attribute_defs(src) {
        let Some(rest) = def.strip_prefix("def ") else {
            continue;
        };
        let Some((_name, decl)) = rest.split_once(" : ") else {
            continue;
        };
        let Some((kind, args)) = decl.split_once('<') else {
            continue;
        };
        if !kind.ends_with("Attr") || kind == "StrBoolAttr" {
            continue;
        }
        // The keyword is the first string literal in the argument list.
        let Some(open) = args.find('"') else { continue };
        let Some(close) = args[open + 1..].find('"') else {
            continue;
        };
        let keyword = args[open + 1..open + 1 + close].to_string();
        if keyword.is_empty() {
            continue;
        }
        out.push(TdAttribute {
            keyword,
            kind: kind.to_string(),
            fn_attr: args.contains("FnAttr"),
            param_attr: args.contains("ParamAttr"),
            ret_attr: args.contains("RetAttr"),
        });
    }
    out
}

/// Probe one attribute in every position `Attributes.td` declares for it,
/// trying each spelling LLVM's grammar allows for its kind. An attribute
/// counts as modeled if *any* spelling parses in *any* declared position —
/// the question is "does the parser know this attribute", not "does it accept
/// one canned form". `align`, `uwtable`, `memory`, `captures`, and
/// `nofpclass` all have bespoke grammars, which is exactly why a single
/// spelling would report false gaps.
fn parser_accepts(attr: &TdAttribute) -> bool {
    let kw = &attr.keyword;
    let mut spellings = vec![kw.clone()];
    match attr.kind.as_str() {
        "IntAttr" => {
            spellings.push(format!("{kw}(8)"));
            spellings.push(format!("{kw} 8"));
            spellings.push(format!("{kw}(none)"));
            spellings.push(format!("{kw}(sync)"));
        }
        "TypeAttr" => spellings.push(format!("{kw}(%s)")),
        "ConstantRangeAttr" => spellings.push(format!("{kw}(i32 0, 10)")),
        "ConstantRangeListAttr" => spellings.push(format!("{kw}((0, 4))")),
        // String-valued attributes are accepted generically as `"key"="value"`.
        "ComplexStrAttr" => spellings.push(format!("\"{kw}\"=\"x\"")),
        _ => {}
    }

    let mut sources = Vec::new();
    for spelled in &spellings {
        if attr.fn_attr {
            sources.push(format!(
                "define void @f() #0 {{ ret void }}\nattributes #0 = {{ {spelled} }}\n"
            ));
        }
        if attr.param_attr {
            sources.push(format!(
                "%s = type {{ i32 }}\ndefine void @f(ptr {spelled} %p) {{ ret void }}\n"
            ));
        }
        if attr.ret_attr {
            sources.push(format!(
                "%s = type {{ i32 }}\ndefine {spelled} ptr @f(ptr %p) {{ ret ptr %p }}\n"
            ));
        }
    }
    if sources.is_empty() {
        return true; // no modeled position to probe
    }
    sources
        .iter()
        .any(|src| parse_dynamic(src.as_str()).is_ok())
}

#[test]
fn vendored_attributes_td_is_parseable() {
    let attrs = parse_attributes_td(ATTRIBUTES_TD);
    assert!(
        attrs.len() > 80,
        "expected to recognise most of Attributes.td's attribute defs, got {}",
        attrs.len()
    );
    // Spot-check the shapes this milestone added, so a change in the `.td`
    // grammar that silently stops matching is caught here rather than showing
    // up as a mysteriously shrinking attribute set.
    let by_kw = |k: &str| attrs.iter().find(|a| a.keyword == k).cloned();
    let uwtable = by_kw("uwtable").expect("uwtable present");
    assert_eq!(uwtable.kind, "IntAttr");
    assert!(uwtable.fn_attr);
    let byval = by_kw("byval").expect("byval present");
    assert_eq!(byval.kind, "TypeAttr");
    assert!(byval.param_attr);

    // The three defs that `Attributes.td` wraps across lines. A reader that
    // works line by line reports the first two as declaring no position at
    // all — which makes the probe below pass vacuously — and does not see the
    // third, so a new attribute could join upstream unnoticed. Pinning them
    // keeps the guard's own reader honest.
    let deref_or_null = by_kw("dereferenceable_or_null").expect("multi-line def is seen");
    assert!(!deref_or_null.fn_attr);
    assert!(deref_or_null.param_attr);
    assert!(deref_or_null.ret_attr);
    let hardening = by_kw("speculative_load_hardening").expect("multi-line def is seen");
    assert!(hardening.fn_attr);
    assert!(!hardening.param_attr);
    let no_create = by_kw("nocreateundeforpoison").expect("multi-line def is seen");
    assert!(no_create.fn_attr);
}

#[test]
fn no_unmodeled_attribute_is_silently_missing() {
    let attrs = parse_attributes_td(ATTRIBUTES_TD);
    let allowed: BTreeSet<&str> = NOT_YET_MODELED.iter().copied().collect();

    let mut missing_and_unlisted = Vec::new();
    for attr in &attrs {
        if allowed.contains(attr.keyword.as_str()) {
            continue;
        }
        if !parser_accepts(attr) {
            missing_and_unlisted.push(format!("{} ({})", attr.keyword, attr.kind));
        }
    }
    missing_and_unlisted.sort();

    assert!(
        missing_and_unlisted.is_empty(),
        "these LLVM 22.1.4 attributes are neither accepted by the parser nor \
         listed in NOT_YET_MODELED — implement them, or add them to the list \
         with intent:\n  {}",
        missing_and_unlisted.join("\n  ")
    );
}

/// Extract the keyword of every `def ... : <Kind><"keyword">;` declaration
/// for a string-attribute `Kind` (`StrBoolAttr` / `ComplexStrAttr`). These
/// defs may put the `: Kind<...>` on a continuation line
/// (`MarkedForWindowsSecureHotPatching` does), so this scans the whole text
/// rather than lines. The `class Kind<string S>` declaration itself is not
/// followed by a `"` and falls out naturally.
fn string_attr_keywords(src: &str, kind: &str) -> Vec<String> {
    let needle = format!("{kind}<\"");
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(open) = rest.find(&needle) {
        rest = &rest[open + needle.len()..];
        let Some(close) = rest.find('"') else { break };
        out.push(rest[..close].to_string());
        rest = &rest[close + 1..];
    }
    out
}

/// Every `StrBoolAttr` declaration in `Attributes.td` must have a
/// `StrBoolAttrKind` variant whose `key` round-trips it, every variant must
/// still name a declared attribute (no stale entries), and the parser must
/// accept the `"key"="true"` spelling as a function attribute. A new
/// upstream `StrBoolAttr` fails here until the reader enum covers it.
///
/// llvmkit-specific drift guard (no upstream counterpart — upstream's
/// `getFnAttribute(...).getValueAsBool()` readers take the raw string);
/// `Attributes.td` is the anchor (D11).
#[test]
fn str_bool_attributes_have_typed_readers() {
    let declared = string_attr_keywords(ATTRIBUTES_TD, "StrBoolAttr");
    assert_eq!(
        declared.len(),
        11,
        "expected the LLVM 22.1.4 StrBoolAttr set, got {declared:?}"
    );

    // Forward: every declaration is covered and parseable.
    let mut missing = Vec::new();
    for keyword in &declared {
        match StrBoolAttrKind::from_key(keyword) {
            Some(kind) => assert_eq!(kind.key(), keyword, "key() must round-trip"),
            None => missing.push(keyword.clone()),
        }
        let src = format!(
            "define void @f() #0 {{ ret void }}\nattributes #0 = {{ \"{keyword}\"=\"true\" }}\n"
        );
        assert!(
            parse_dynamic(src.as_str()).is_ok(),
            "parser rejects string attribute \"{keyword}\"=\"true\""
        );
    }
    assert!(
        missing.is_empty(),
        "these Attributes.td StrBoolAttr declarations have no StrBoolAttrKind \
         variant — extend the reader enum:\n  {}",
        missing.join("\n  ")
    );

    // Reverse: no variant names an attribute upstream no longer declares.
    let declared_set: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    const ALL_VARIANTS: [StrBoolAttrKind; 11] = [
        StrBoolAttrKind::MarkedForWindowsHotPatching,
        StrBoolAttrKind::AllowDirectAccessInHotPatchFunction,
        StrBoolAttrKind::LessPreciseFpmad,
        StrBoolAttrKind::NoInfsFpMath,
        StrBoolAttrKind::NoNansFpMath,
        StrBoolAttrKind::NoSignedZerosFpMath,
        StrBoolAttrKind::NoJumpTables,
        StrBoolAttrKind::NoInlineLineTables,
        StrBoolAttrKind::ProfileSampleAccurate,
        StrBoolAttrKind::UseSampleProfile,
        StrBoolAttrKind::LoaderReplaceable,
    ];
    for variant in ALL_VARIANTS {
        assert!(
            declared_set.contains(variant.key()),
            "StrBoolAttrKind::{variant:?} names {:?}, which LLVM 22.1.4 does \
             not declare as a StrBoolAttr (typo, or upstream removed it)",
            variant.key()
        );
    }
}

/// The `ComplexStrAttr` set is exactly the two denormal modes, which llvmkit
/// types via `DenormalMode` (`FunctionValue::denormal_mode_raw` /
/// `denormal_mode_f32_raw`) rather than a reader enum. A new upstream
/// `ComplexStrAttr` fails here until it gets a typed reader of its own.
/// The generic string-attribute probe in
/// `no_unmodeled_attribute_is_silently_missing` already covers parser
/// acceptance for these.
///
/// llvmkit-specific drift guard; `Attributes.td` is the anchor (D11).
#[test]
fn complex_str_attributes_are_typed() {
    let declared = string_attr_keywords(ATTRIBUTES_TD, "ComplexStrAttr");
    assert_eq!(
        declared,
        ["denormal-fp-math", "denormal-fp-math-f32"],
        "ComplexStrAttr set changed — give the new attribute a typed reader"
    );
}

#[test]
fn not_yet_modeled_list_has_no_stale_entries() {
    let attrs = parse_attributes_td(ATTRIBUTES_TD);
    let known: BTreeSet<&str> = attrs.iter().map(|a| a.keyword.as_str()).collect();

    let mut stale = Vec::new();
    for entry in NOT_YET_MODELED {
        if !known.contains(entry) {
            stale.push(*entry);
        }
    }
    assert!(
        stale.is_empty(),
        "NOT_YET_MODELED names attributes that LLVM 22.1.4 does not define \
         (typo, or upstream removed them): {stale:?}"
    );
}
