//! `!DIAssignID !N` as an *instruction attachment*, in both orderings.
//!
//! **No upstream counterpart, and deliberately so.** Nothing in
//! `llvm/test/Assembler` or `llvm/test/DebugInfo` exercises a `!DIAssignID`
//! instruction attachment through `llvm-as` alone — the assignment-tracking
//! fixtures all run a pipeline — and this file exists to pin llvmkit's own
//! answer rather than to claim upstream's.
//!
//! What upstream does here, read from `LLParser.cpp` (**read, not run** — no
//! `llvm-as` was executed for this note): `parseInstructionMetadata` pushes to
//! `TempDIAssignIDAttachments[N]` whenever `MDK == LLVMContext::MD_DIAssignID`
//! and never calls `Inst.setMetadata` on that path, while the drain that
//! replays those instructions lives inside `parseStandaloneMetadata`'s
//! `ForwardRefMDNodes.find` hit. On that reading, an attachment whose `!N` was
//! **already defined earlier in the file** is pushed and never drained, so
//! `llvm-as` drops it; only the forward-referenced ordering survives. That
//! reading is why the two orderings are pinned separately here.
//!
//! llvmkit has no `MD_DIAssignID` branch at all: it resolves metadata forward
//! references by reserve-then-fill on a stable `MetadataId`, so
//! `skip_trailing_metadata` calls `inst.set_metadata(..)` for every kind and
//! the attachment survives in both orderings. Nothing observable is lost by the
//! omission today — llvmkit models no `AssignmentIDToInstrs` side map for the
//! special case to protect.

use llvmkit_asmparser::parser;

fn parse_and_print(source: &str) -> String {
    let module = parser::parse_dynamic(source).expect("fixture parses");
    let verified = module.verify().expect("fixture verifies");
    format!("{verified}")
}

/// Forward-referenced: `!0` is defined *after* the function. This is the
/// ordering upstream's `TempDIAssignIDAttachments` drain covers, so both sides
/// keep the attachment.
#[test]
fn a_forward_referenced_assign_id_attachment_survives() {
    let printed = parse_and_print(
        "\
define void @f() {
  %a = alloca i32, align 4, !DIAssignID !0
  ret void
}

!0 = distinct !DIAssignID()
",
    );
    assert!(
        printed.contains("!DIAssignID !0"),
        "the attachment must survive:\n{printed}"
    );
}

/// Already defined: `!0` is defined *before* the function. llvmkit keeps the
/// attachment here too. On the reading of `LLParser.cpp` above, `llvm-as`
/// drops it — that half is a reading and has not been executed, so this test
/// asserts only llvmkit's side and names the difference rather than encoding
/// an unverified upstream answer.
#[test]
fn an_already_defined_assign_id_attachment_also_survives_here() {
    let printed = parse_and_print(
        "\
!0 = distinct !DIAssignID()

define void @f() {
  %a = alloca i32, align 4, !DIAssignID !0
  ret void
}
",
    );
    assert!(
        printed.contains("!DIAssignID !0"),
        "llvmkit keeps the attachment in this ordering too:\n{printed}"
    );
}
