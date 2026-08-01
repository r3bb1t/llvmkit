//! Ports of the `impliesPoison` and `propagatesPoison` fixtures from
//! `llvm/unittests/Analysis/ValueTrackingTest.cpp` — tranche 2 of the
//! `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Upstream's `ValueTrackingTest` fixture parses a snippet and picks named
//! values out of it (`A`, `A2`). These tests do the same through llvmkit's
//! parser, so the IR under test is upstream's text verbatim rather than
//! rebuilt through the builder.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, Module, Unverified, Value, ValueTrackingQuery, implies_poison, propagates_poison,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    parser::parse_dynamic(source).expect("fixture parses")
}

/// The instruction named `%name` in the module's single function.
fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|f| f.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// Run `implies_poison(%lhs, %rhs)` over a parsed snippet.
fn implies(source: &str, lhs: &str, rhs: &str) -> bool {
    let module = parse(source);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::new(&data_layout);
    implies_poison(named(&module, lhs), named(&module, rhs), &query).expect("implies_poison")
}

/// Every `impliesPoisonTest_*` fixture, with upstream's expectation.
///
/// Ports `TEST_F(ValueTrackingTest, impliesPoisonTest_Identity)` and its seven
/// siblings — `ICmp`, `ICmpUnknown`, `AddNswOkay`, `AddNswOkay2`, `AddNsw`,
/// `Cmp`, `AddSubSameOps` — from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`. The IR is upstream's,
/// unchanged.
#[test]
fn implies_poison_fixtures() {
    // (name, source, lhs, rhs, upstream expectation)
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "Identity",
            "define void @test(i32 %x, i32 %y) {\n  %A = add i32 %x, %y\n  ret void\n}\n",
            "A",
            "A",
            true,
        ),
        (
            "ICmp",
            "define void @test(i32 %x) {\n  %A2 = icmp eq i32 %x, 0\n  %A = icmp eq i32 %x, 1\n  ret void\n}\n",
            "A2",
            "A",
            true,
        ),
        (
            "ICmpUnknown",
            "define void @test(i32 %x, i32 %y) {\n  %A2 = icmp eq i32 %x, %y\n  %A = icmp eq i32 %x, 1\n  ret void\n}\n",
            "A2",
            "A",
            false,
        ),
        (
            "AddNswOkay",
            "define void @test(i32 %x) {\n  %A2 = add nsw i32 %x, 1\n  %A = add i32 %A2, 1\n  ret void\n}\n",
            "A2",
            "A",
            true,
        ),
        (
            "AddNswOkay2",
            "define void @test(i32 %x) {\n  %A2 = add i32 %x, 1\n  %A = add nsw i32 %A2, 1\n  ret void\n}\n",
            "A2",
            "A",
            true,
        ),
        (
            "AddNsw",
            "define void @test(i32 %x) {\n  %A2 = add nsw i32 %x, 1\n  %A = add i32 %x, 1\n  ret void\n}\n",
            "A2",
            "A",
            false,
        ),
        (
            "Cmp",
            "define void @test(i32 %x, i32 %y, i1 %c) {\n  %A2 = icmp eq i32 %x, %y\n  %A0 = icmp ult i32 %x, %y\n  %A = or i1 %A0, %c\n  ret void\n}\n",
            "A2",
            "A",
            true,
        ),
        (
            "AddSubSameOps",
            "define void @test(i32 %x, i32 %y, i1 %c) {\n  %A2 = add i32 %x, %y\n  %A = sub i32 %x, %y\n  ret void\n}\n",
            "A2",
            "A",
            true,
        ),
    ];

    for (name, source, lhs, rhs, expected) in cases {
        assert_eq!(
            implies(source, lhs, rhs),
            *expected,
            "impliesPoisonTest_{name}: implies_poison(%{lhs}, %{rhs})"
        );
    }
}

/// `propagatesPoison` over the opcode classes upstream's `TEST(ValueTracking,
/// propagatesPoison)` table covers, restricted to those llvmkit models.
///
/// Upstream indexes a `Use`; llvmkit has no `Use` type, so the user and the
/// operand position are passed separately. The expectations are upstream's:
/// binary operators, casts, `icmp`/`fcmp` and `getelementptr` propagate from
/// every operand; `freeze` and `phi` never do; `select` propagates only from
/// its condition.
#[test]
fn propagates_poison_by_opcode_class() {
    let source = "\
define void @test(i32 %x, i32 %y, i1 %c, ptr %p, float %f) {
  %bin = add i32 %x, %y
  %cast = trunc i32 %x to i8
  %cmp = icmp eq i32 %x, %y
  %fcmp = fcmp oeq float %f, %f
  %gep = getelementptr i32, ptr %p, i32 %x
  %fr = freeze i32 %x
  %sel = select i1 %c, i32 %x, i32 %y
  %neg = fneg float %f
  ret void
}
";
    let module = parse(source);

    // (instruction, operand index, upstream expectation)
    let cases: &[(&str, usize, bool)] = &[
        ("bin", 0, true),
        ("bin", 1, true),
        ("cast", 0, true),
        ("cmp", 0, true),
        ("cmp", 1, true),
        ("fcmp", 0, true),
        ("gep", 0, true),
        ("gep", 1, true),
        ("neg", 0, true),
        // `freeze` is exactly the instruction that stops poison.
        ("fr", 0, false),
        // Only the select's condition propagates; an unselected arm's poison
        // does not reach the result.
        ("sel", 0, true),
        ("sel", 1, false),
        ("sel", 2, false),
    ];

    for (name, operand_index, expected) in cases {
        assert_eq!(
            propagates_poison(named(&module, name), *operand_index),
            *expected,
            "propagates_poison(%{name}, operand {operand_index})"
        );
    }
}

/// A `phi` never propagates poison from an incoming value, because the value
/// only reaches the result along one edge. Upstream lists `PHI` beside
/// `Freeze` and `Invoke` in `propagatesPoison`'s first arm.
#[test]
fn phi_never_propagates_poison() {
    let source = "\
define i32 @test(i1 %c, i32 %x, i32 %y) {
entry:
  br i1 %c, label %a, label %b
a:
  br label %join
b:
  br label %join
join:
  %p = phi i32 [ %x, %a ], [ %y, %b ]
  ret i32 %p
}
";
    let module = parse(source);
    let phi = named(&module, "p");
    assert!(!propagates_poison(phi, 0));
    assert!(!propagates_poison(phi, 1));
}
