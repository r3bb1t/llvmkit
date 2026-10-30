//! Anti-drift guard: every place the crate wraps a slot as a `Constant` is
//! audited.
//!
//! `Constant::from_parts` is crate-private and takes any `Value`, so the type
//! system does not stop it wrapping an argument, an instruction or a block.
//! `Constant::kind` relies on it never doing so: a global's own slot is a
//! `GlobalValue`, and every other kind is unreachable. That is a property of
//! the call sites, so this test pins them — each function that calls
//! `Constant::from_parts`, how many times, and why the slot it wraps is a
//! constant or a global value. A new call, or a second one in an audited
//! function, fails here until it is audited and listed.
//!
//! No upstream counterpart: LLVM's `Constant *` is a C++ type that cannot hold
//! an `Argument`.

use std::path::{Path, PathBuf};

/// Checks `ValueKindData::Constant(_)` on the slot before wrapping it.
const GUARDED: &str = "checks ValueKindData::Constant before wrapping";
/// Wraps a constant it has just interned.
const INTERNED: &str = "wraps a constant it has just interned";
/// Wraps an element or operand of a constant. Those are constants or global
/// values: a constant is built from constants, and RAUW admits nothing else
/// into one (`constants.rs::is_constant_value`).
const ELEMENT: &str = "wraps an element or operand of a constant";
/// Wraps a global value's own slot, or a constant-or-global a global stores
/// (an aliasee, a resolver, a personality, prefix or prologue, an initializer).
const GLOBAL: &str = "wraps a global value, or a constant or global it stores";
/// Re-mints a constant handle it was given.
const REMINT: &str = "re-mints a constant handle it was given";

/// `(file under crates/llvmkit-ir/src, enclosing function, call count, why)`.
const AUDITED: &[(&str, &str, usize, &str)] = &[
    ("constant.rs", "aggregate_element", 1, ELEMENT),
    ("constant.rs", "constant_expression_splat_value", 1, ELEMENT),
    ("constant.rs", "element_at", 1, ELEMENT),
    ("constant.rs", "into_constant", 1, REMINT),
    ("constant.rs", "splat_value", 1, ELEMENT),
    ("constant_fold.rs", "aggregate_elements", 1, ELEMENT),
    ("constant_fold.rs", "constant_from_id", 1, GUARDED),
    ("constant_folding.rs", "build_canonical_i8_gep", 1, INTERNED),
    ("constant_folding.rs", "constant_fold_constant", 1, INTERNED),
    (
        "constant_folding.rs",
        "constant_id_guaranteed_not_to_be_undef_or_poison",
        1,
        GUARDED,
    ),
    ("constant_folding.rs", "fold_int_to_ptr_pair", 1, ELEMENT),
    ("constant_folding.rs", "fold_ptr_to_int_pair", 1, ELEMENT),
    ("constant_folding.rs", "peel_one_gep_level", 1, INTERNED),
    ("constants.rs", "as_constant", 4, REMINT),
    ("constants.rs", "constant_handle", 1, INTERNED),
    ("function.rs", "as_aggregate_ptr", 1, INTERNED),
    ("function.rs", "as_global_constant_ptr", 1, INTERNED),
    ("function.rs", "personality_fn", 1, GLOBAL),
    ("function.rs", "prefix_data", 1, GLOBAL),
    ("function.rs", "prologue_data", 1, GLOBAL),
    ("global_alias.rs", "aliasee", 1, GLOBAL),
    ("global_alias.rs", "as_constant", 1, GLOBAL),
    ("global_ifunc.rs", "as_constant", 1, GLOBAL),
    ("global_ifunc.rs", "resolver", 1, GLOBAL),
    ("global_variable.rs", "as_constant", 1, GLOBAL),
    ("global_variable.rs", "as_global_constant_ptr", 1, INTERNED),
    (
        "global_variable.rs",
        "as_global_constant_ptr_offset",
        1,
        INTERNED,
    ),
    ("global_variable.rs", "initializer", 1, GLOBAL),
    ("global_variable.rs", "ptr_offset", 1, INTERNED),
    ("global_variable.rs", "try_delta_from", 1, INTERNED),
    ("global_variable.rs", "try_delta_from_plus", 1, INTERNED),
    ("pointer_analysis.rs", "find_inserted_value", 1, GUARDED),
    ("pointer_analysis.rs", "is_null_constant", 1, GUARDED),
    ("vector_utils.rs", "constant_mask_lanes_satisfy", 1, GUARDED),
    ("vector_utils.rs", "find_scalar_element", 2, GUARDED),
    ("vector_utils.rs", "is_splat_value_at_depth", 1, GUARDED),
    (
        "vector_utils.rs",
        "possibly_demanded_elements_in_mask",
        1,
        GUARDED,
    ),
    ("vector_utils.rs", "splat_value", 1, GUARDED),
];

/// The mint's call spelling; the definition reads `fn from_parts`.
const MINT_CALL: &str = "Constant::from_parts(";

/// Every `.rs` file under `dir`, recursively, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// The name of the function whose body holds line `index`: the nearest
/// earlier line that declares one, comments skipped.
fn enclosing_function(lines: &[&str], index: usize) -> Option<String> {
    lines[..=index].iter().rev().find_map(|line| {
        let code = line.trim_start();
        if code.starts_with("//") {
            return None;
        }
        let after = code.split_once("fn ")?.1;
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// `function -> call count` for every `Constant::from_parts` call in one
/// file's `lines`, comments skipped.
fn mint_calls(file: &str, lines: &[&str]) -> Vec<(String, usize)> {
    let mut calls: Vec<(String, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || !code.contains(MINT_CALL) {
            continue;
        }
        let function = enclosing_function(lines, index)
            .unwrap_or_else(|| panic!("{file}: a mint outside any function"));
        match calls
            .iter_mut()
            .find(|(call_function, _)| *call_function == function)
        {
            Some((_, count)) => *count += 1,
            None => calls.push((function, 1)),
        }
    }
    calls
}

/// `(file, function) -> call count` for every `Constant::from_parts` call.
fn mint_sites(source_root: &Path) -> Vec<(String, String, usize)> {
    let mut sites: Vec<(String, String, usize)> = Vec::new();
    for path in rust_files(source_root) {
        let source = std::fs::read_to_string(&path).expect("source file is readable");
        let lines: Vec<&str> = source.lines().collect();
        let file = path
            .strip_prefix(source_root)
            .expect("under the source root")
            .to_string_lossy()
            .replace('\\', "/");
        for (function, count) in mint_calls(&file, &lines) {
            sites.push((file.clone(), function, count));
        }
    }
    sites
}

/// Every `Constant::from_parts` call sits in an audited function, as many
/// times as audited, and every audited function still holds its calls.
/// llvmkit-specific (D10): the guard behind `Constant::kind`'s `unreachable!`.
#[test]
fn every_constant_handle_mint_is_audited() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = mint_sites(&source_root);
    let mut problems = Vec::new();
    for (file, function, count) in &found {
        match AUDITED
            .iter()
            .find(|(audited_file, audited_function, _, _)| {
                audited_file == file && audited_function == function
            }) {
            None => problems.push(format!("unaudited: {file}::{function} ({count} call(s))")),
            Some((_, _, audited_count, _)) if audited_count != count => problems.push(format!(
                "{file}::{function}: {count} call(s), audited {audited_count}"
            )),
            Some(_) => {}
        }
    }
    for (file, function, _, _) in AUDITED {
        if !found
            .iter()
            .any(|(found_file, found_function, _)| found_file == file && found_function == function)
        {
            problems.push(format!("stale audit entry: {file}::{function}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The scanner counts each call against its enclosing function, and skips a
/// comment that merely names the mint. llvmkit-specific: the guard's own
/// positive control, run on the same per-file routine the guard runs.
#[test]
fn the_mint_scanner_finds_calls_and_skips_comments() {
    let lines = [
        "fn audited_one(slot: ValueSlot) {",
        "    // Constant::from_parts( in a comment",
        "    let c = Constant::from_parts(value);",
        "}",
        "pub(crate) fn audited_two<'ctx>(value: Value<'ctx>) {",
        "    let a = Constant::from_parts(value);",
        "    // fn not_a_function_header",
        "    let b = Constant::from_parts(value);",
        "}",
    ];
    assert_eq!(
        mint_calls("synthetic.rs", &lines),
        [("audited_one".to_owned(), 1), ("audited_two".to_owned(), 2)]
    );
}
