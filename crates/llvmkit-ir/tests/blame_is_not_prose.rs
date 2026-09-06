//! Blame -- caller error, host limit, or llvmkit invariant -- is carried by the
//! variant and its rustdoc, never by words inside a message.
//!
//! **llvmkit-specific, no upstream counterpart.** Upstream states these
//! invariants as `assert`s (`PHINode::Create`, `llvm/lib/IR/Instructions.cpp`,
//! takes the result type as an argument), so there is no upstream error value
//! to port and nothing to compare against. This is a source scan for the same
//! reason the branch it guards is unreachable: the defect cannot be provoked at
//! runtime, only written.

const PASS_CONTEXT_RS: &str = include_str!("../src/pass_context.rs");
const IR_BUILDER_RS: &str = include_str!("../src/ir_builder.rs");
const CONSTANTS_RS: &str = include_str!("../src/constants.rs");

/// No diagnostic message names its own blame class in prose.
#[test]
fn no_message_spells_its_blame_class() {
    for (name, source) in [
        ("pass_context.rs", PASS_CONTEXT_RS),
        ("ir_builder.rs", IR_BUILDER_RS),
        ("constants.rs", CONSTANTS_RS),
    ] {
        let offenders: Vec<&str> = source
            .lines()
            .filter(|line| line.contains("internal invariant") && line.contains('"'))
            .collect();
        assert!(
            offenders.is_empty(),
            "{name} spells a blame class inside a message, where no consumer can \
             branch on it:\n{}",
            offenders.join("\n")
        );
    }
}
