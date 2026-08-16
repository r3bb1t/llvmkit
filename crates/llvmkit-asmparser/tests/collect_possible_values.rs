//! `llvm::collectPossibleValues` — enumerating the constants a value can take.
//!
//! **No upstream unit test exists.** The function's only in-tree caller is
//! `SimplifyCFG`, which reaches it through `.ll` regression tests of a pass
//! llvmkit does not have, so these fixtures are llvmkit's. What they assert is
//! read straight off the upstream implementation: the walk crosses `select`
//! and `phi`, stops at an immediate constant, skips a recurrence phi's
//! self-edge, and gives up on anything else or on exceeding `MaxCount`.
//!
//! The assertions are on *cardinality*, not on which constants came back.
//! Upstream collects into a `SmallPtrSet` whose iteration order is
//! unspecified, so pinning an order here would invent a contract; the count
//! and the complete/incomplete answer are the parts upstream actually
//! promises.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DynBrand, Module, Unverified, Value, ValueTrackingQuery, collect_possible_values,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

fn named<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
        .to_erased()
}

/// Collect from `%name`, with upstream's default `AllowUndefOrPoison = true`.
fn collect(source: &str, name: &str, max_count: usize) -> Option<usize> {
    let module = parse(source);
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    collect_possible_values(named(&module, name), max_count, true, &query)
        .expect("query succeeds")
        .map(|values| values.len())
}

/// A `select` between two constants has exactly those two values, and a nested
/// one has three — the worklist arm upstream spells as `Instruction::Select`.
#[test]
fn a_select_contributes_both_of_its_arms() {
    assert_eq!(
        collect(
            r"
define i32 @two(i1 %c) {
  %sel = select i1 %c, i32 7, i32 9
  ret i32 %sel
}
",
            "sel",
            4,
        ),
        Some(2)
    );

    assert_eq!(
        collect(
            r"
define i32 @three(i1 %c, i1 %d) {
  %inner = select i1 %d, i32 7, i32 9
  %sel = select i1 %c, i32 11, i32 %inner
  ret i32 %sel
}
",
            "sel",
            4,
        ),
        Some(3)
    );
}

/// A repeated constant is inserted once: upstream checks set membership before
/// spending a slot against `MaxCount`, so `select c, 7, 7` needs one, not two.
#[test]
fn a_repeated_constant_costs_one_slot() {
    assert_eq!(
        collect(
            r"
define i32 @repeat(i1 %c) {
  %sel = select i1 %c, i32 7, i32 7
  ret i32 %sel
}
",
            "sel",
            1,
        ),
        Some(1)
    );
}

/// The `Instruction::PHI` arm, including its fast path: an incoming value that
/// is the phi itself is skipped rather than pushed.
#[test]
fn a_phi_contributes_its_incomings_and_skips_its_own_recurrence() {
    assert_eq!(
        collect(
            r"
define i32 @loop(i1 %c) {
entry:
  br label %head
head:
  %p = phi i32 [ 3, %entry ], [ %p, %head ]
  br i1 %c, label %head, label %exit
exit:
  ret i32 %p
}
",
            "p",
            4,
        ),
        Some(1)
    );
}

/// Exceeding `MaxCount` is "incomplete", which is the `None`. Upstream returns
/// `false` with the set left partially filled; a caller must not read it, so
/// there is nothing to hand back.
#[test]
fn exceeding_the_bound_answers_incomplete() {
    let source = r"
define i32 @three(i1 %c, i1 %d) {
  %inner = select i1 %d, i32 7, i32 9
  %sel = select i1 %c, i32 11, i32 %inner
  ret i32 %sel
}
";
    assert_eq!(collect(source, "sel", 3), Some(3));
    assert_eq!(collect(source, "sel", 2), None);
}

/// Any leaf that is neither an immediate constant nor a `select`/`phi` makes
/// the set unenumerable — upstream's `default: return false` and the tail of
/// its `Push` lambda.
#[test]
fn an_unenumerable_leaf_gives_up() {
    // An argument: `Push` falls through to `return false`.
    assert_eq!(
        collect(
            r"
define i32 @arg(i1 %c, i32 %x) {
  %sel = select i1 %c, i32 7, i32 %x
  ret i32 %sel
}
",
            "sel",
            4,
        ),
        None
    );

    // An opcode the switch does not name.
    assert_eq!(
        collect(
            r"
define i32 @adds(i32 %x) {
  %sum = add i32 %x, 1
  ret i32 %sum
}
",
            "sum",
            4,
        ),
        None
    );
}
