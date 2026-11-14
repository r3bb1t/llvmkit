//! Anti-drift guard for the builder's terminator law.
//!
//! A builder a function pass's context lends (`InPass`) must not emit a
//! terminator. The compiler holds that for every terminator build in the
//! impls bounded `S: Terminating`, and for every build that takes its block
//! through `into_block_to_terminate`. Nothing in the compiler stops a new
//! `&self` build from appending a terminator payload through the shared
//! `append_instruction` in a `S: Positioning` impl, which a lent builder
//! reaches. This test is that tie: in `ir_builder.rs`, every construction of a
//! terminator payload sits in an impl whose header bounds `S: Terminating`.
//!
//! The terminator kinds are read from `InstructionKindData::is_terminator` in
//! `instruction.rs`, so a new terminator kind is covered without editing this
//! file. No upstream counterpart: LLVM's `IRBuilder` has no positioning
//! typestate.

use std::path::Path;

/// The source of `relative`, under this crate's `src/`.
fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative);
    std::fs::read_to_string(&path).expect("source file reads")
}

/// The variant names `InstructionKindData::is_terminator` matches, read from
/// its `matches!` arms (`Self::Ret(_)`, `| Self::Br(_)`, …).
fn terminator_kinds(instruction_rs: &str) -> Vec<String> {
    let start = instruction_rs
        .find("pub(super) fn is_terminator(&self) -> bool {")
        .expect("`InstructionKindData::is_terminator` is in instruction.rs");
    let body = &instruction_rs[start..];
    let end = body.find("\n    }\n").expect("the fn body ends");
    body[..end]
        .split("Self::")
        .skip(1)
        .filter_map(|arm| arm.split('(').next())
        .map(str::to_owned)
        .collect()
}

/// The 1-based lines of `source` that build one of `kinds` outside an impl
/// whose header bounds `S: Terminating`, and the number of such builds seen.
///
/// A build is a code line (not a `//` comment) containing
/// `InstructionKindData::Kind(`. Its impl is the nearest `impl` line above at
/// column 0; the header is that line through the first line ending in `{`.
fn builds_outside_terminating_impls(source: &str, kinds: &[String]) -> (Vec<usize>, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut outside = Vec::new();
    let mut builds = 0;
    let mut in_terminating_impl = false;
    let mut in_header = false;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("impl") {
            in_terminating_impl = false;
            in_header = true;
        }
        if in_header {
            if line.trim() == "S: Terminating," || line.contains(" S: Terminating") {
                in_terminating_impl = true;
            }
            if line.trim_end().ends_with('{') {
                in_header = false;
            }
        }
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let builds_terminator = kinds
            .iter()
            .any(|kind| code.contains(&format!("InstructionKindData::{kind}(")));
        if builds_terminator {
            builds += 1;
            if !in_terminating_impl {
                outside.push(index + 1);
            }
        }
    }
    (outside, builds)
}

/// Every terminator payload `ir_builder.rs` builds sits in an impl bounded
/// `S: Terminating`, so a lent `InPass` builder can reach none of them.
/// Positive control: the kind reader finds `Ret` and `Unreachable`, and the
/// scanner flags a build in a `S: Positioning` impl and accepts one in a
/// `S: Terminating` impl, in synthetic sources; and the tree has builds to
/// check, so a rename cannot make the test pass vacuously.
///
/// No upstream counterpart: llvmkit-specific regression for the lent-builder
/// terminator law (`docs/design/layout-token-design.md` decision 9).
#[test]
fn every_terminator_payload_is_built_in_a_terminating_impl() {
    let kinds = terminator_kinds(&source("instruction.rs"));
    assert!(
        kinds.iter().any(|kind| kind == "Ret") && kinds.iter().any(|kind| kind == "Unreachable"),
        "the terminator kinds were not read: {kinds:?}"
    );

    // Positive control: the scanner itself.
    let positioning = "impl<S> X<S>\nwhere\n    S: Positioning,\n{\n    fn f(&self) {\n        self.append_instruction(t, InstructionKindData::Ret(p), \"\");\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(positioning, &kinds),
        (vec![6], 1)
    );
    let terminating = "impl<S> X<S>\nwhere\n    S: Terminating,\n{\n    fn f(self) {\n        self.append_instruction(t, InstructionKindData::Br(p), \"\");\n    }\n}\n";
    assert_eq!(
        builds_outside_terminating_impls(terminating, &kinds),
        (Vec::new(), 1)
    );

    let (outside, builds) = builds_outside_terminating_impls(&source("ir_builder.rs"), &kinds);
    assert!(
        builds > 0,
        "no terminator build found in ir_builder.rs: was it renamed?"
    );
    assert!(
        outside.is_empty(),
        "terminator payloads built outside a `S: Terminating` impl, at ir_builder.rs lines {outside:?}"
    );
}
