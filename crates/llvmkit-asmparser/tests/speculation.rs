//! Ports of the speculation-safety and UB-reachability predicates — tranche 6
//! of the `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! Every fixture below comes from `llvm/unittests/Analysis/ValueTrackingTest.cpp`,
//! IR inlined verbatim and the same API called on the same values:
//!
//! - `TEST(ValueTracking, GuaranteedToTransferExecutionToSuccessor)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfPoison)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfPoisonSelect)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfUndefOrPoison)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_exploitBranchCond)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_phi)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBeUndefOrPoison_splat)`

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    DominatorTree, DynBrand, InstructionView, Module, Unverified, Value, ValueTrackingQuery,
    is_guaranteed_to_transfer_execution_to_successor, is_known_not_poison,
    is_known_not_undef_or_poison, program_undefined_if_poison,
    program_undefined_if_undef_or_poison,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// The instruction named `%name` in the module's single definition.
fn named_instruction<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> InstructionView<'m, DynBrand> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

fn named_value<'m>(module: &'m Module<DynBrand, Unverified>, name: &str) -> Value<'m, DynBrand> {
    named_instruction(module, name).to_erased()
}

/// A dominator tree for the module's first *definition*, which is what
/// upstream's `DominatorTree DT(*F)` builds.
///
/// Skipping declarations matters: every fixture here opens with one, and a
/// declaration has no entry block, so a tree built from it is empty and every
/// dominance query silently answers false.
fn dominator_tree(module: &Module<DynBrand, Unverified>) -> DominatorTree {
    let ids: Vec<_> = module
        .as_view()
        .functions()
        .filter(|function| function.basic_blocks().next().is_some())
        .map(|function| function.id())
        .collect();
    let id = *ids.first().expect("fixture defines a function body");
    DominatorTree::new(module.view(id))
}

/// `TEST(ValueTracking, GuaranteedToTransferExecutionToSuccessor)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`.
///
/// The assembly and the thirteen expected answers are upstream's, in order.
/// The one deviation is spelling: LLVM 22 no longer parses the legacy
/// `readonly` / `argmemonly` function attributes, which the fixture predates,
/// so those are written as the `memory(...)` forms LLVM canonicalises them to —
/// `readonly` is `memory(read)` and `argmemonly` is `memory(argmem: readwrite)`.
/// Every answer stays what upstream asserts, because the predicate reads
/// `nounwind` and `willreturn`, never the memory effects.
#[test]
fn guaranteed_to_transfer_execution_to_successor() {
    let module = parse(
        r"
declare void @nounwind_readonly(ptr) nounwind memory(read)
declare void @nounwind_argmemonly(ptr) nounwind memory(argmem: readwrite)
declare void @nounwind_willreturn(ptr) nounwind willreturn
declare void @throws_but_readonly(ptr) memory(read)
declare void @throws_but_argmemonly(ptr) memory(argmem: readwrite)
declare void @throws_but_willreturn(ptr) willreturn

declare void @unknown(ptr)

define void @f(ptr %p) {
  call void @nounwind_readonly(ptr %p)
  call void @nounwind_argmemonly(ptr %p)
  call void @nounwind_willreturn(ptr %p)
  call void @throws_but_readonly(ptr %p)
  call void @throws_but_argmemonly(ptr %p)
  call void @throws_but_willreturn(ptr %p)
  call void @unknown(ptr %p) nounwind memory(read)
  call void @unknown(ptr %p) nounwind memory(argmem: readwrite)
  call void @unknown(ptr %p) nounwind willreturn
  call void @unknown(ptr %p) memory(read)
  call void @unknown(ptr %p) memory(argmem: readwrite)
  call void @unknown(ptr %p) willreturn
  ret void
}
",
    );

    let expected = [
        false, // call void @nounwind_readonly(ptr %p)
        false, // call void @nounwind_argmemonly(ptr %p)
        true,  // call void @nounwind_willreturn(ptr %p)
        false, // call void @throws_but_readonly(ptr %p)
        false, // call void @throws_but_argmemonly(ptr %p)
        false, // call void @throws_but_willreturn(ptr %p)
        false, // call void @unknown(ptr %p) nounwind memory(read)
        false, // call void @unknown(ptr %p) nounwind memory(argmem: readwrite)
        true,  // call void @unknown(ptr %p) nounwind willreturn
        false, // call void @unknown(ptr %p) memory(read)
        false, // call void @unknown(ptr %p) memory(argmem: readwrite)
        false, // call void @unknown(ptr %p) willreturn
        false, // ret void
    ];

    let view = module.as_view();
    let entry = view
        .functions()
        .find(|function| function.name() == "f")
        .expect("fixture defines @f")
        .basic_blocks()
        .next()
        .expect("@f has an entry block");

    let answers: Vec<bool> = entry
        .instructions()
        .map(|instruction| is_guaranteed_to_transfer_execution_to_successor(&instruction))
        .collect();
    assert_eq!(answers, expected);
}

/// `TEST_F(ValueTrackingTest, programUndefinedIfPoison)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// If `%A` were poison then `%B` is poison whatever `%mask` holds, and the
/// `udiv` by a poison divisor is UB.
#[test]
fn program_undefined_if_poison_through_or() {
    let module = parse(
        r"
declare i32 @any_num()

define void @test(i32 %mask) {
  %A = call i32 @any_num()
  %B = or i32 %A, %mask
  %C = udiv i32 1, %B
  ret void
}
",
    );
    let a = named_instruction(&module, "A");
    assert!(program_undefined_if_poison(&a));
}

/// `TEST_F(ValueTrackingTest, programUndefinedIfPoisonSelect)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// This is the arm the operand walk cannot reach on its own: if `%A` is poison
/// so is `%B`, and a `select` whose *both* arms are poison is poison however the
/// condition goes.
#[test]
fn program_undefined_if_poison_through_select() {
    let module = parse(
        r"
declare i32 @any_num()

define void @test(i1 %Cond) {
  %A = call i32 @any_num()
  %B = add i32 %A, 1
  %C = select i1 %Cond, i32 %A, i32 %B
  %D = udiv i32 1, %C
  ret void
}
",
    );
    let a = named_instruction(&module, "A");
    assert!(program_undefined_if_poison(&a));
}

/// `TEST_F(ValueTrackingTest, programUndefinedIfUndefOrPoison)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// The same fixture as [`program_undefined_if_poison_through_or`] with the
/// other question asked: undef does not propagate eagerly, so `%A` being undef
/// and `%mask` being 1 makes the `udiv` well defined.
#[test]
fn program_undefined_if_undef_or_poison_through_or() {
    let module = parse(
        r"
declare i32 @any_num()

define void @test(i32 %mask) {
  %A = call i32 @any_num()
  %B = or i32 %A, %mask
  %C = udiv i32 1, %B
  ret void
}
",
    );
    let a = named_instruction(&module, "A");
    assert!(!program_undefined_if_undef_or_poison(&a));
}

/// `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_exploitBranchCond)`
/// from `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// `%A` feeds the branch condition, so reaching either successor at all proves
/// `%A` was not poison — the dominating-condition walk.
#[test]
fn is_known_not_poison_exploits_branch_condition() {
    let module = parse(
        r"
declare i1 @any_bool()

define void @test(i1 %y) {
  %A = call i1 @any_bool()
  %cond = and i1 %A, %y
  br i1 %cond, label %BB1, label %BB2

BB1:
  ret void

BB2:
  ret void
}
",
    );
    let dominator_tree = dominator_tree(&module);
    let data_layout = module.data_layout();
    let a = named_value(&module, "A");

    let view = module.as_view();
    let blocks: Vec<_> = view
        .functions()
        .flat_map(|function| function.basic_blocks())
        .collect();
    let (entry, rest) = blocks.split_first().expect("fixture has blocks");
    assert_eq!(entry.instruction_count(), 3, "entry is the branching block");

    for block in rest {
        let terminator = block
            .instructions()
            .next_back()
            .expect("every block is terminated");
        let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
            .with_dominator_tree(&dominator_tree)
            .with_context_instruction(&terminator);
        assert!(
            is_known_not_poison(a, &query).expect("query succeeds"),
            "is_known_not_poison does not hold at the terminator of a successor block"
        );
    }
}

/// `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_phi)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// The phi is its own context instruction: `%A` reaches `%cond` through
/// `%A.next`, and the branch on `%cond` dominates the loop header on the
/// back edge.
#[test]
fn is_known_not_poison_loop_phi() {
    let module = parse(
        r"
declare i32 @any_i32(i32)

define void @test() {
ENTRY:
  br label %LOOP

LOOP:
  %A = phi i32 [ 0, %ENTRY ], [ %A.next, %NEXT ]
  %A.next = call i32 @any_i32(i32 %A)
  %cond = icmp eq i32 %A.next, 0
  br i1 %cond, label %NEXT, label %EXIT

NEXT:
  br label %LOOP

EXIT:
  ret void
}
",
    );
    let dominator_tree = dominator_tree(&module);
    let data_layout = module.data_layout();
    let a = named_instruction(&module, "A");
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
        .with_dominator_tree(&dominator_tree)
        .with_context_instruction(&a);
    assert!(is_known_not_poison(a.to_erased(), &query).expect("query succeeds"));
}

/// The dominating-condition arm of `isGuaranteedNotToBeUndefOrPoison`, in
/// isolation.
///
/// **The IR has no upstream counterpart.** The two upstream tests named for
/// this arm — `isGuaranteedNotToBePoison_exploitBranchCond` and
/// `isGuaranteedNotToBePoison_phi`, both ported above — are in fact satisfied
/// by the *earlier* `programUndefinedIfUndefOrPoison` arm, in LLVM as much as
/// here: their branch on `%A` sits in the same block as `%A`, where the
/// in-block scan already reaches it. Deleting the idom walk leaves both green,
/// so neither covers it.
///
/// The **oracle is upstream**, which states the rule and its shape in the
/// comment above the `Dominator = DNode->getIDom()` loop
/// (`ValueTracking.cpp`): "If V is used as a branch condition before reaching
/// CtxI, V cannot be undef or poison." This fixture is that shape with the
/// earlier arm ruled out — `%A`'s own block ends on a *different* condition and
/// has two successors, so the in-block scan stops there.
#[test]
fn is_known_not_poison_via_dominating_branch_in_another_block() {
    let module = parse(
        r"
declare i1 @any_bool()
declare void @may_throw()

define void @test(i1 %other) {
entry:
  %A = call i1 @any_bool()
  br i1 %other, label %check, label %exit

check:
  br i1 %A, label %then, label %exit

then:
  call void @may_throw()
  ret void

exit:
  ret void
}
",
    );
    let dominator_tree = dominator_tree(&module);
    let data_layout = module.data_layout();
    let a = named_value(&module, "A");

    let view = module.as_view();
    let context = view
        .functions()
        .flat_map(|function| function.basic_blocks())
        .find(|block| block.name().as_deref() == Some("then"))
        .expect("fixture defines %then")
        .instructions()
        .next()
        .expect("%then is not empty");

    // Without the context instruction there is nothing to dominate, so the arm
    // cannot fire and no earlier arm proves it either.
    let unanchored = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(
        !is_known_not_poison(a, &unanchored).expect("query succeeds"),
        "no arm but the dominating-condition walk can prove this fixture"
    );

    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
        .with_dominator_tree(&dominator_tree)
        .with_context_instruction(&context);
    assert!(is_known_not_poison(a, &query).expect("query succeeds"));
}

/// `TEST_F(ValueTrackingTest, isGuaranteedNotToBeUndefOrPoison_splat)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR unchanged.
///
/// Only the splatted value has to be checked, and it is a `noundef` parameter.
#[test]
fn is_known_not_undef_or_poison_splat() {
    let module = parse(
        r"
define <4 x i32> @test(i32 noundef %x) {
  %ins = insertelement <4 x i32> poison, i32 %x, i32 0
  %A = shufflevector <4 x i32> %ins, <4 x i32> poison, <4 x i32> zeroinitializer
  ret <4 x i32> %A
}
",
    );
    let data_layout = module.data_layout();
    let a = named_value(&module, "A");
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(is_known_not_undef_or_poison(a, &query).expect("query succeeds"));
}
