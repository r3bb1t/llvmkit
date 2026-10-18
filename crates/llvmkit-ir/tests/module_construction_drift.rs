//! Anti-drift guard for `UniqueBrand`'s soundness argument.
//!
//! `UniqueBrand` is a safe trait: implementing it for a brand claims that at
//! most one live module carries that brand. The claim holds because module
//! storage is allocated in exactly two places — `Module::registered`, behind
//! `Module::branded` and `Module::branded_once`, which claims the brand in the
//! process-wide registry before it hands the module out, and `Module::dynamic`,
//! whose `DynBrand` is no `UniqueBrand` — and because neither `Module` nor its
//! storage can be cloned. Nothing in the compiler ties a new constructor to the
//! registry, so this test is that tie.
//!
//! No upstream counterpart: an LLVM `Module` has no brand.

use std::path::{Path, PathBuf};

/// The storage constructor's call spelling.
const CORE_NEW: &str = "ModuleCore::new(";

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

/// The name of the function whose body holds line `index`: the nearest line
/// at or above it that declares one, comments skipped.
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

/// The enclosing function of each [`CORE_NEW`] call in `source`, comments
/// skipped; `None` for a call outside any function.
fn constructor_calls(source: &str) -> Vec<Option<String>> {
    let lines: Vec<&str> = source.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            !code.starts_with("//") && code.contains(CORE_NEW)
        })
        .map(|(index, _)| enclosing_function(&lines, index))
        .collect()
}

/// Whether `source` makes the type declared by `declaration` cloneable: an
/// `impl … Clone for <name>` anywhere, or a `#[derive(…Clone…)]` among the
/// attribute lines directly above the declaration.
fn is_cloneable(source: &str, name: &str, declaration: &str) -> bool {
    let impl_name = format!("Clone for {name}");
    if source.lines().any(|line| {
        let code = line.trim_start();
        code.starts_with("impl")
            && (code.contains(&format!("{impl_name}<")) || code.contains(&format!("{impl_name} ")))
    }) {
        return true;
    }
    let lines: Vec<&str> = source.lines().collect();
    lines
        .iter()
        .position(|line| line.trim_start().starts_with(declaration))
        .is_some_and(|at| {
            lines[..at]
                .iter()
                .rev()
                .map(|line| line.trim_start())
                .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
                .any(|line| line.starts_with("#[derive(") && line.contains("Clone"))
        })
}

/// Module storage is created only inside `fn registered` and `fn dynamic`
/// (`crates/llvmkit-ir/src/module.rs`), and neither `Module` nor `ModuleCore`
/// is `Clone` — the premise `UniqueBrand`'s soundness argument rests on.
///
/// llvmkit-specific guard (D7), no upstream counterpart. Positive control:
/// `the_construction_scanner_names_each_call_and_sees_a_clone`.
#[test]
fn only_the_registry_and_dynamic_create_modules() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut calls = Vec::new();
    for path in rust_files(&source_root) {
        let source = std::fs::read_to_string(&path).expect("source file is readable");
        let file = path
            .strip_prefix(&source_root)
            .expect("under the source root")
            .to_string_lossy()
            .replace('\\', "/");
        for function in constructor_calls(&source) {
            calls.push((file.clone(), function));
        }
    }
    calls.sort();
    assert_eq!(
        calls,
        [
            ("module.rs".to_owned(), Some("dynamic".to_owned())),
            ("module.rs".to_owned(), Some("registered".to_owned())),
        ],
        "module storage may be created only by `Module::registered` and `Module::dynamic`"
    );

    let module =
        std::fs::read_to_string(source_root.join("module.rs")).expect("module.rs is readable");
    assert!(
        !is_cloneable(&module, "Module", "pub struct Module<"),
        "`Module` must not be `Clone`: a clone is a second live module of its brand"
    );
    assert!(
        !is_cloneable(&module, "ModuleCore", "pub(super) struct ModuleCore"),
        "`ModuleCore` must not be `Clone`"
    );
}

/// The scanner names each storage-constructor call by its enclosing function,
/// skips one in a comment, and sees a `Clone` by impl and by derive, on
/// synthetic sources. llvmkit-specific: the guard's own positive control.
#[test]
fn the_construction_scanner_names_each_call_and_sees_a_clone() {
    let source = [
        "fn registered() {",
        "    // ModuleCore::new( in a comment",
        "    let core = Box::new(ModuleCore::new(name));",
        "}",
        "pub(crate) fn sneaky() -> Module<B> {",
        "    Module { core: Box::new(ModuleCore::new(name)) }",
        "}",
    ]
    .join("\n");
    assert_eq!(
        constructor_calls(&source),
        [Some("registered".to_owned()), Some("sneaky".to_owned())]
    );

    let derived = "/// Doc.\n#[derive(Debug, Clone)]\npub struct Module<B> {}";
    assert!(is_cloneable(derived, "Module", "pub struct Module<"));
    let implemented = "pub struct Module<B> {}\nimpl<B> Clone for Module<B> {}";
    assert!(is_cloneable(implemented, "Module", "pub struct Module<"));
    let neither = "#[derive(Debug)]\npub struct Module<B> {}";
    assert!(!is_cloneable(neither, "Module", "pub struct Module<"));
}
