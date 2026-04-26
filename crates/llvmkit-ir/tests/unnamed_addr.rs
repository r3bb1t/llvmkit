//! Round-trip tests for the [`UnnamedAddr`] marker through the
//! AsmWriter. Mirrors the test block in
//! `llvm/test/Assembler/unnamed_addr.ll`.

use llvmkit_ir::{Linkage, Module, RVoid, UnnamedAddr};

fn declare(name: &str, value: UnnamedAddr) -> String {
    let m = Module::new("u");
    let void = m.void_type();
    let fn_ty = m.fn_type(void.as_type(), Vec::<llvmkit_ir::Type>::new(), false);
    let f = m
        .function_builder::<RVoid>(name, fn_ty)
        .linkage(Linkage::External)
        .unnamed_addr(value)
        .build()
        .expect("build");
    let _ = f;
    format!("{m}")
}

#[test]
fn default_emits_no_unnamed_addr_token() {
    let text = declare("plain", UnnamedAddr::None);
    assert!(!text.contains("unnamed_addr"), "got:\n{text}");
}

#[test]
fn local_emits_local_unnamed_addr() {
    let text = declare("local", UnnamedAddr::Local);
    assert!(
        text.contains("declare void @local() local_unnamed_addr\n"),
        "got:\n{text}"
    );
}

#[test]
fn global_emits_unnamed_addr() {
    let text = declare("global", UnnamedAddr::Global);
    assert!(
        text.contains("declare void @global() unnamed_addr\n"),
        "got:\n{text}"
    );
    // `unnamed_addr` and `local_unnamed_addr` are *different* keywords;
    // the global form should not contain the local prefix.
    assert!(!text.contains("local_unnamed_addr"), "got:\n{text}");
}
