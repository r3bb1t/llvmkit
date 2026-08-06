//! Anti-drift guard for the fixed metadata attachment kinds.
//!
//! `MetadataAttachmentKind` hand-mirrors the `LLVM_FIXED_MD_KIND` entries of
//! LLVM's `FixedMetadataKinds.def`, and that is how 17 kinds (from `irr_loop`
//! through `implicit.ref`) went missing for several releases: nothing tied
//! the enum to its source. Upstream cannot drift because
//! `LLVMContext::LLVMContext` registers the fixed kinds by `#include`-ing the
//! `.def` directly (`lib/IR/LLVMContext.cpp`).
//!
//! This test gives the same guarantee: it parses the vendored
//! `FixedMetadataKinds.def` and asserts every entry has a named
//! `MetadataAttachmentKind` variant whose `fixed_id` and `name` agree with
//! the `.def`. A new upstream kind, a renamed one, or a renumbered one fails
//! CI. The `.def` is vendored under this crate's `tablegen/` (tracked, unlike
//! `orig_cpp/`), so the guard runs everywhere the tests do — the same
//! arrangement `attribute_td_drift.rs` uses for `Attributes.td`.
//!
//! llvmkit-specific: upstream has no equivalent test because its registration
//! is generated; the `.def` itself is the upstream anchor (D11).

use llvmkit_ir::MetadataAttachmentKind;

const FIXED_METADATA_KINDS_DEF: &str =
    include_str!("../tablegen/llvm-22.1.4/include/llvm/IR/FixedMetadataKinds.def");

/// One `LLVM_FIXED_MD_KIND(EnumID, Name, Value)` entry as the `.def` spells
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FixedKind {
    name: String,
    value: u32,
}

/// Parse the `LLVM_FIXED_MD_KIND(...)` invocations. Entries may span lines
/// (`MD_mem_parallel_loop_access` does), so this scans the whole text rather
/// than lines. The `#error` guard line also contains the macro name but no
/// string literal, which is the filter that skips it.
fn parse_fixed_metadata_kinds(src: &str) -> Vec<FixedKind> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(open) = rest.find("LLVM_FIXED_MD_KIND(") {
        rest = &rest[open + "LLVM_FIXED_MD_KIND(".len()..];
        let Some(close) = rest.find(')') else { break };
        let arguments = &rest[..close];
        rest = &rest[close + 1..];

        // `EnumID, "Name", Value` — the guard's `#error` text has no quotes.
        let Some(quote_open) = arguments.find('"') else {
            continue;
        };
        let Some(quote_close) = arguments[quote_open + 1..].find('"') else {
            continue;
        };
        let name = arguments[quote_open + 1..quote_open + 1 + quote_close].to_string();
        let Some((_, value_text)) = arguments.rsplit_once(',') else {
            continue;
        };
        let Ok(value) = value_text.trim().parse::<u32>() else {
            continue;
        };
        out.push(FixedKind { name, value });
    }
    out
}

/// The `.def` grammar is stable: 47 entries whose values are exactly
/// `0..=46`, in order. A change in the vendored file's shape fails here
/// rather than showing up as a mysteriously shrinking kind set.
#[test]
fn vendored_fixed_metadata_kinds_def_is_parseable() {
    let kinds = parse_fixed_metadata_kinds(FIXED_METADATA_KINDS_DEF);
    assert_eq!(
        kinds.len(),
        47,
        "expected the LLVM 22.1.4 fixed metadata kinds, got {kinds:?}"
    );
    for (expected_value, kind) in (0u32..).zip(kinds.iter()) {
        assert_eq!(
            kind.value, expected_value,
            "fixed metadata kind values must be dense and in order: {kind:?}"
        );
    }
    // Spot-check the one multi-line entry so a change in the `.def` layout
    // that silently drops it is caught by name.
    assert!(
        kinds
            .iter()
            .any(|k| k.name == "llvm.mem.parallel_loop_access" && k.value == 10),
        "multi-line entry MD_mem_parallel_loop_access not parsed: {kinds:?}"
    );
}

/// Every `LLVM_FIXED_MD_KIND` entry must have a named
/// `MetadataAttachmentKind` variant (`from_name` must not fall back to
/// `Custom`), its `fixed_id` must match the `.def` value, and its `name`
/// must round-trip the `.def` spelling.
#[test]
fn every_fixed_metadata_kind_has_a_matching_variant() {
    let kinds = parse_fixed_metadata_kinds(FIXED_METADATA_KINDS_DEF);
    assert!(!kinds.is_empty(), "vendored .def parsed to nothing");

    let mut drifted = Vec::new();
    for FixedKind { name, value } in &kinds {
        let variant = MetadataAttachmentKind::from_name(name);
        match variant.fixed_id() {
            None => drifted.push(format!(
                "{name} ({value}): no named variant, parses as Custom"
            )),
            Some(id) if id != *value => {
                drifted.push(format!("{name}: fixed_id says {id}, the .def says {value}"))
            }
            Some(_) => {}
        }
        if variant.name() != name {
            drifted.push(format!(
                "{name}: name() round-trips as {:?}",
                variant.name()
            ));
        }
    }
    assert!(
        drifted.is_empty(),
        "MetadataAttachmentKind drifted from FixedMetadataKinds.def:\n  {}",
        drifted.join("\n  ")
    );

    // The open remainder of the namespace carries no fixed ID.
    assert_eq!(
        MetadataAttachmentKind::from_name("not.a.fixed.kind").fixed_id(),
        None
    );
}
