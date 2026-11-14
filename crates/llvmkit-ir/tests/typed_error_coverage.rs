//! Every variant of the typed error enums that replaced
//! `IrError::InvalidOperation` is asserted by some test.
//!
//! A typed error is only as good as the test that pins it at its raise site.
//! Several variants share one `Display` (three `GepConstantError` variants
//! print "invalid getelementptr constant expression"), so a test that compares
//! text cannot tell them apart; a raise site that picks the wrong variant is
//! caught only by a test that names the variant. This test reads each enum's
//! variants out of `src/error.rs` and requires each to be spelled
//! `Enum::Variant` in some file under the workspace crates' `tests/`
//! directories, other than this one.
//!
//! What the scan reads, so its reach is not overstated: a variant counts as
//! asserted when its path appears in a test file's code (comment lines are
//! skipped). It does not check that the
//! appearance is an assertion of a result. The exemptions below name the
//! variants no public entry can reach, each with the reason.
//!
//! **No upstream counterpart**: an llvmkit-specific guard on its own error
//! surface, behind `CLAUDE.md`'s "No loose types" and "Resolve, don't patch"
//! rules.

use std::path::{Path, PathBuf};

const ERROR_RS: &str = include_str!("../src/error.rs");

/// The enums held to the rule, by name.
const ENUMS: [&str; 5] = [
    "ConstantError",
    "PtrAuthRequirement",
    "ConstantExprError",
    "GepConstantError",
    "BuilderError",
];

/// Variants a test need not name, each with the reason: one no public entry
/// reaches, or a wrapper whose payload enum is itself in [`ENUMS`]. A variant
/// here that a test also names is reported, so the exemption is removed once
/// it is no longer needed.
const EXEMPT: [(&str, &str, &str); 2] = [
    (
        "BuilderError",
        "SplatLaneCountTooWide",
        "a NonZeroU32 lane count always fits a 32- or 64-bit usize",
    ),
    (
        "ConstantExprError",
        "GetElementPtr",
        "a wrapper: tests build it through `From<GepConstantError>`, and its \
         GepConstantError payload is held to the rule",
    ),
];

/// The variant names of `pub enum <name>` in `source`: lines indented four
/// spaces inside the enum's body that start with an uppercase letter.
fn variants<'s>(source: &'s str, name: &str) -> Vec<&'s str> {
    let header = format!("pub enum {name} {{");
    let Some(start) = source.find(&header) else {
        panic!("src/error.rs declares `pub enum {name}`");
    };
    let body = &source[start + header.len()..];
    let end = body.find("\n}").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            if !rest.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            let length = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            Some(&rest[..length])
        })
        .collect()
}

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

/// The text of every test file in the workspace crates but this one.
fn test_sources() -> String {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(&crates) else {
        panic!("the workspace's crates directory is readable");
    };
    for entry in entries.flatten() {
        rust_files(&entry.path().join("tests"), &mut files);
    }
    files
        .iter()
        .filter(|path| !path.ends_with("typed_error_coverage.rs"))
        .map(|path| {
            let source = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
            code_only(&source)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `source` without its comment lines, so a variant a comment mentions does
/// not count as asserted.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The variant reader finds the variants of a synthetic enum, skipping its
/// doc comments, attributes and struct-variant fields.
///
/// **No upstream counterpart**: the positive control for
/// [`every_typed_error_variant_is_asserted`]'s reader.
#[test]
fn the_variant_reader_sees_each_variant() {
    let source = "\
pub enum Sample {
    /// A unit variant.
    #[error(\"a\")]
    Unit,
    /// A struct variant.
    Fields {
        /// A field.
        field: u32,
    },
    Wrapper(#[from] Other),
}
pub enum Next {
    Later,
}
";
    assert_eq!(variants(source, "Sample"), ["Unit", "Fields", "Wrapper"]);
}

/// Each variant of [`ENUMS`] is named by some test, except the exempt ones,
/// and no exempt variant is named by one.
///
/// **No upstream counterpart**: llvmkit-specific, see the module docs.
#[test]
fn every_typed_error_variant_is_asserted() {
    let tests = test_sources();
    let mut failures = Vec::new();
    for name in ENUMS {
        let found = variants(ERROR_RS, name);
        assert!(!found.is_empty(), "`{name}` has variants");
        for variant in found {
            let path = format!("{name}::{variant}");
            let exempt = EXEMPT.iter().any(|(enum_name, exempt_variant, _)| {
                *enum_name == name && *exempt_variant == variant
            });
            let named = tests.contains(&path);
            if !exempt && !named {
                failures.push(format!("{path} is asserted by no test"));
            } else if exempt && named {
                failures.push(format!(
                    "{path} is exempt but a test names it: drop the exemption"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
