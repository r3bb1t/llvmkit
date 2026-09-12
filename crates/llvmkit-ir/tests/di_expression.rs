//! Ports of the `DIExpression` fixtures from
//! `llvm/unittests/IR/MetadataTest.cpp` in the vendored `llvmorg-22.1.4` tree.
//!
//! Upstream builds a `DIExpression` from a raw `uint64_t Elements[]` array and
//! asks `isValid()`. llvmkit stores a `DIExpression` body as source spellings
//! (`DwarfExpressionOperand`) and recovers the element array through
//! `metadata::expression_elements`, so the port drives
//! `metadata::expression_is_valid` — the port of `DIExpression::isValid` — with
//! the same element arrays, written the same way.
//!
//! The `dwarf::DW_OP_*` constants upstream names are looked up through
//! `llvmkit_ir::dwarf::operation_encoding`, which is the drift-locked
//! transcription of the same `Dwarf.def` file.

use llvmkit_ir::dwarf::operation_encoding;
use llvmkit_ir::metadata::{DwarfExpressionOperand, expression_elements, expression_is_valid};

/// `dwarf::DW_OP_<name>` as a `u64` element, by the name upstream writes.
fn op(name: &str) -> u64 {
    u64::from(
        operation_encoding(name)
            .unwrap_or_else(|| panic!("`{name}` is a DW_OP spelling `Dwarf.def` carries")),
    )
}

/// Port of `TEST_F(DIExpressionTest, isValid)`
/// (`llvm/unittests/IR/MetadataTest.cpp`). Upstream's `EXPECT_VALID` /
/// `EXPECT_INVALID` macros each build a `uint64_t Elements[]` and assert
/// `DIExpression::get(Context, Elements)->isValid()`; here the same arrays go
/// straight to the ported predicate.
#[test]
fn is_valid() {
    let expect_valid = |elements: &[u64]| {
        assert!(
            expression_is_valid(elements),
            "expected a valid expression: {elements:?}"
        );
    };
    let expect_invalid = |elements: &[u64]| {
        assert!(
            !expression_is_valid(elements),
            "expected an invalid expression: {elements:?}"
        );
    };

    // Empty expression should be valid.
    assert!(expression_is_valid(&[]));

    // Valid constructions.
    expect_valid(&[op("DW_OP_plus_uconst"), 6]);
    expect_valid(&[op("DW_OP_constu"), 6, op("DW_OP_plus")]);
    expect_valid(&[op("DW_OP_deref")]);
    expect_valid(&[op("DW_OP_LLVM_fragment"), 3, 7]);
    expect_valid(&[op("DW_OP_plus_uconst"), 6, op("DW_OP_deref")]);
    expect_valid(&[op("DW_OP_deref"), op("DW_OP_plus_uconst"), 6]);
    expect_valid(&[op("DW_OP_deref"), op("DW_OP_LLVM_fragment"), 3, 7]);
    expect_valid(&[
        op("DW_OP_deref"),
        op("DW_OP_plus_uconst"),
        6,
        op("DW_OP_LLVM_fragment"),
        3,
        7,
    ]);
    expect_valid(&[op("DW_OP_LLVM_entry_value"), 1]);
    expect_valid(&[op("DW_OP_LLVM_arg"), 0, op("DW_OP_LLVM_entry_value"), 1]);

    // Invalid constructions.
    expect_invalid(&[u64::from(u32::MAX)]);
    expect_invalid(&[op("DW_OP_plus"), 0]);
    expect_invalid(&[op("DW_OP_plus_uconst")]);
    expect_invalid(&[op("DW_OP_LLVM_fragment")]);
    expect_invalid(&[op("DW_OP_LLVM_fragment"), 3]);
    expect_invalid(&[op("DW_OP_LLVM_fragment"), 3, 7, op("DW_OP_plus_uconst"), 3]);
    expect_invalid(&[op("DW_OP_LLVM_fragment"), 3, 7, op("DW_OP_deref")]);
    expect_invalid(&[op("DW_OP_LLVM_entry_value"), 2]);
    expect_invalid(&[op("DW_OP_plus_uconst"), 5, op("DW_OP_LLVM_entry_value"), 1]);
    expect_invalid(&[
        op("DW_OP_LLVM_arg"),
        0,
        op("DW_OP_plus_uconst"),
        5,
        op("DW_OP_LLVM_entry_value"),
        1,
    ]);
    expect_invalid(&[op("DW_OP_LLVM_arg"), 1, op("DW_OP_LLVM_entry_value"), 1]);
}

/// No upstream counterpart: upstream's `DIExpression` stores `uint64_t`
/// elements, so it has no spelling-to-element step to test. This pins the one
/// llvmkit adds — `DwarfExpressionOperand::element` and
/// `metadata::expression_elements` — against the encodings
/// `LLParser::parseDIExpressionBody` obtains from `dwarf::getOperationEncoding`
/// and `getAttributeEncoding` for the same source text.
#[test]
fn operand_spellings_map_to_upstream_elements() {
    let operands = [
        DwarfExpressionOperand::Operation("DW_OP_LLVM_convert".to_owned()),
        DwarfExpressionOperand::Literal(16),
        DwarfExpressionOperand::Operation("DW_ATE_signed".to_owned()),
    ];
    assert_eq!(
        expression_elements(&operands),
        Some(vec![
            op("DW_OP_LLVM_convert"),
            16,
            u64::from(
                llvmkit_ir::dwarf::attribute_encoding("DW_ATE_signed").expect("DW_ATE_signed")
            )
        ])
    );

    // A spelling neither table carries has no element at all. Only the IR API
    // can build one: `parse_di_expression_body` rejects it by name.
    let unknown = [DwarfExpressionOperand::Operation(
        "DW_OP_nonsense".to_owned(),
    )];
    assert_eq!(expression_elements(&unknown), None);
}
