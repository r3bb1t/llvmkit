//! Ports of the speculation-safety and UB-reachability predicates — tranche 6
//! of the `ValueTracking.h` port (see `docs/future-work.md`).
//!
//! The ported fixtures come from `llvm/unittests/Analysis/ValueTrackingTest.cpp`,
//! IR inlined verbatim and the same API called on the same values:
//!
//! - `TEST(ValueTracking, GuaranteedToTransferExecutionToSuccessor)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfPoison)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfPoisonSelect)`
//! - `TEST_F(ValueTrackingTest, programUndefinedIfUndefOrPoison)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_exploitBranchCond)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBePoison_phi)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBeUndefOrPoison)`
//! - `TEST_F(ValueTrackingTest, isGuaranteedNotToBeUndefOrPoison_splat)`
//! - `TEST(ValueTracking, canCreatePoisonOrUndef)`
//!
//! and from `llvm/test/Transforms/InstSimplify/freeze.ll`, the functions
//! `@icmp`, `@icmp_noopt`, `@fcmp` and `@fcmp_noopt`, each asked the question
//! `simplifyFreezeInst` asks.
//!
//! A test with no upstream counterpart says so in its own doc comment.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    Capability, DominatorTree, Dyn, DynBrand, InstructionView, IrBuilder, Module, NoFolder,
    TerminatorKind, Unpositioned, Unverified, User, Value, ValueTrackingQuery, VectorType,
    can_create_poison, can_create_undef_or_poison,
    is_guaranteed_to_transfer_execution_to_successor, is_known_not_poison,
    is_known_not_undef_or_poison, lanes, program_undefined_if_poison,
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
) -> InstructionView<'m, DynBrand, llvmkit_ir::ReadOnly> {
    module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

fn named_value<'m>(
    module: &'m Module<DynBrand, Unverified>,
    name: &str,
) -> Value<'m, DynBrand, llvmkit_ir::ReadOnly> {
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
        .find(|function| function.name().as_deref() == Some("f"))
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

/// The allocated-object arm sees through a zero-offset `inbounds`
/// `getelementptr`, which is what `stripPointerCastsSameRepresentation` is
/// there for.
///
/// **The IR has no upstream counterpart**; the oracle is the comment upstream
/// puts on the strip: "the stripped pointer is checked as it has to be pointing
/// into an allocated object or be `null` to ensure `inbounds` getelement
/// pointers with a zero offset could not produce poison."
///
/// Without the strip the GEP is `inbounds`, so
/// `can_create_undef_or_poison_kind` reports it poison-capable and the operand
/// walk is skipped — `%zero` answers false where upstream answers true.
#[test]
fn is_known_not_undef_or_poison_strips_a_zero_offset_inbounds_gep() {
    let module = parse(
        r"
define void @test() {
  %a = alloca i32
  %zero = getelementptr inbounds i32, ptr %a, i64 0
  %past = getelementptr inbounds i32, ptr %a, i64 1
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);

    assert!(is_known_not_undef_or_poison(named_value(&module, "zero"), &query).expect("succeeds"));
    // A non-zero offset is not stripped, so it stays poison-capable.
    assert!(!is_known_not_undef_or_poison(named_value(&module, "past"), &query).expect("succeeds"));
}

/// `isGuaranteedNotToBeUndefOrPoison(V)`, with no context.
fn well_defined<'ctx, C: Capability>(
    value: Value<'ctx, DynBrand, C>,
    query: &ValueTrackingQuery<'_, 'ctx, DynBrand>,
) -> bool {
    is_known_not_undef_or_poison(value, query).expect("query succeeds")
}

/// `isGuaranteedNotToBePoison(V)`, with no context.
fn not_poison<'ctx, C: Capability>(
    value: Value<'ctx, DynBrand, C>,
    query: &ValueTrackingQuery<'_, 'ctx, DynBrand>,
) -> bool {
    is_known_not_poison(value, query).expect("query succeeds")
}

/// `TEST_F(ValueTrackingTest, isGuaranteedNotToBeUndefOrPoison)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, IR and assertions
/// unchanged. Upstream's fixture string runs the declaration into the
/// definition with no newline between them; one is written here.
///
/// `%A` is a `bitcast` of `%x`, so it is well defined when `%x` is. `%x`
/// carries no attribute, and the call after `%A` passes it to a `noundef`
/// parameter, so the program is undefined unless `%x` is well defined. That is
/// `programUndefinedIfUndefOrPoison`, which upstream's routine reaches for an
/// argument as for an instruction: its `dyn_cast<Operator>` arm guards only
/// the arms inside it.
#[test]
fn is_known_not_undef_or_poison_of_an_argument_and_of_constants() {
    let module = parse(
        r"
declare void @f(i32 noundef)
define void @test(i32 %x) {
  %A = bitcast i32 %x to i32
  call void @f(i32 noundef %x)
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);

    assert!(well_defined(named_value(&module, "A"), &query));
    let i8_ty = module.i8_type().as_type();
    assert!(!well_defined(i8_ty.undef().as_erased(), &query));
    assert!(!well_defined(i8_ty.poison().as_erased(), &query));
    assert!(not_poison(i8_ty.undef().as_erased(), &query));
    assert!(!not_poison(i8_ty.poison().as_erased(), &query));

    let i32_ty = module.i32_type();
    let undef = i32_ty.as_type().undef().as_constant();
    let poison = i32_ty.as_type().poison().as_constant();
    let one = i32_ty.const_int(1_i32).as_constant();
    let two = i32_ty.const_int(2_i32).as_constant();
    let pair = VectorType::of(i32_ty, lanes::<2>());

    let v1 = pair.const_vector([one, two]).expect("a vector").as_erased();
    assert!(well_defined(v1, &query));
    assert!(not_poison(v1, &query));

    let v2 = pair
        .const_vector([one, undef])
        .expect("a vector")
        .as_erased();
    assert!(!well_defined(v2, &query));
    assert!(not_poison(v2, &query));

    let v3 = pair
        .const_vector([one, poison])
        .expect("a vector")
        .as_erased();
    assert!(!well_defined(v3, &query));
    assert!(!not_poison(v3, &query));
}

/// The dominating-condition walk proves an argument, as it proves an
/// instruction.
///
/// **The IR has no upstream counterpart**: it is the fixture of
/// `is_known_not_poison_via_dominating_branch_in_another_block` with the value
/// branched on made a parameter. Upstream's routine runs that walk for every
/// value its earlier arms do not answer; only its `dyn_cast<Operator>` arm is
/// restricted to operators. The oracle is upstream's comment above the
/// `Dominator = DNode->getIDom()` loop (`ValueTracking.cpp`): "If V is used as
/// a branch condition before reaching CtxI, V cannot be undef or poison." The
/// unanchored query is the control: `%a`'s block, the entry, ends on another
/// condition with two successors, so the in-block scan of
/// `programUndefinedIfUndefOrPoison` cannot prove it.
#[test]
fn is_known_not_poison_of_an_argument_via_a_dominating_branch() {
    let module = parse(
        r"
declare void @may_throw()

define void @test(i1 %other, i1 %a) {
entry:
  br i1 %other, label %check, label %exit

check:
  br i1 %a, label %then, label %exit

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
    let function = module.function_dyn("test").expect("fixture defines @test");
    let a = module
        .view(function)
        .param(1)
        .expect("@test has a second parameter")
        .as_erased();

    let view = module.as_view();
    let context = view
        .functions()
        .flat_map(|function| function.basic_blocks())
        .find(|block| block.name().as_deref() == Some("then"))
        .expect("fixture defines %then")
        .instructions()
        .next()
        .expect("%then is not empty");

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

/// The rows `(can create poison?, can create undef?, operand)`, each operand
/// frozen as `%rN` in `@f`, over `@g` and `@h`.
fn frozen_operands(rows: &[((bool, bool), &str)]) -> Module<DynBrand, Unverified> {
    let mut source = String::from(
        "@g = global i32 0\n@h = global i32 0\n\ndefine void @f(<vscale x 4 x i32> %svx, i32 %x) {\n",
    );
    for (index, (_, operand)) in rows.iter().enumerate() {
        source.push_str(&format!("  %r{index} = freeze {operand}\n"));
    }
    source.push_str("  ret void\n}\n");
    parse(&source)
}

/// `canCreatePoison` and `canCreateUndefOrPoison` of a constant expression.
///
/// **No upstream counterpart**: `TEST(ValueTracking, canCreatePoisonOrUndef)`
/// in `llvm/unittests/Analysis/ValueTrackingTest.cpp` asks only instructions.
/// This test has its shape, rows of `(can create poison?, can create undef?,
/// IR)` and its two assertions, over the expressions a llvmkit
/// `ConstantExpr` can be, with one row for each arm of `canCreateUndefOrPoison`
/// an expression reaches. The oracle is that routine's arms and
/// `Operator::hasPoisonGeneratingFlags`: `nuw`, `nsw`, `inbounds` and
/// `inrange` are poison-generating flags, an `addrspacecast` can create poison,
/// another cast or a binary operator cannot, and an `insertelement` can when its
/// index is not below the vector's known minimum length. The `insertelement`
/// rows are scalable because a fixed-width one folds when it is parsed
/// (`ConstantFoldInsertElementInstruction` declines only a scalable vector).
///
/// Each operand is printed first, so a row the parser folded away fails as a
/// different operand rather than as a wrong answer.
#[test]
fn can_create_poison_of_a_constant_expression() {
    let rows: [((bool, bool), &str); 12] = [
        (
            (false, false),
            "i64 add (i64 ptrtoint (ptr @g to i64), i64 1)",
        ),
        (
            (true, false),
            "i64 add nuw (i64 ptrtoint (ptr @g to i64), i64 1)",
        ),
        (
            (true, false),
            "i64 sub nsw (i64 ptrtoint (ptr @g to i64), i64 ptrtoint (ptr @h to i64))",
        ),
        (
            (false, false),
            "i64 xor (i64 ptrtoint (ptr @g to i64), i64 1)",
        ),
        ((false, false), "ptr getelementptr (i8, ptr @g, i64 1)"),
        (
            (true, false),
            "ptr getelementptr inbounds (i8, ptr @g, i64 1)",
        ),
        (
            (true, false),
            "ptr getelementptr inrange(0, 4) (i8, ptr @g, i64 1)",
        ),
        ((false, false), "i64 ptrtoint (ptr @g to i64)"),
        (
            (false, false),
            "float bitcast (i32 ptrtoint (ptr @g to i32) to float)",
        ),
        (
            (true, false),
            "ptr addrspace(1) addrspacecast (ptr @g to ptr addrspace(1))",
        ),
        (
            (false, false),
            "<vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 ptrtoint (ptr @g to i32), i64 3)",
        ),
        (
            (true, false),
            "<vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 ptrtoint (ptr @g to i32), i64 4)",
        ),
    ];
    let module = frozen_operands(&rows);
    for (index, ((poison, undef), written)) in rows.iter().enumerate() {
        let operand = named_instruction(&module, &format!("r{index}"))
            .operand(0)
            .expect("a freeze has an operand");
        assert_eq!(
            &operand.to_string(),
            written,
            "row {index} parses as written"
        );
        assert_eq!(
            can_create_poison(operand, true),
            *poison,
            "canCreatePoison of row {index}: {written}"
        );
        assert_eq!(
            can_create_undef_or_poison(operand, true),
            *undef || *poison,
            "canCreateUndefOrPoison of row {index}: {written}"
        );
    }

    // `ConsiderFlagsAndMetadata` gates the flags: without it, the `add nuw`
    // and the `inbounds` getelementptr answer as their opcodes do.
    for index in [1, 5] {
        let operand = named_instruction(&module, &format!("r{index}"))
            .operand(0)
            .expect("a freeze has an operand");
        assert!(!can_create_poison(operand, false), "row {index}");
    }
}

/// The scalable splat `ConstantVector::getSplat` builds of a value that is no
/// null, `undef` or `poison`, `shufflevector (insertelement (poison, V, 0),
/// poison, zeroinitializer)`, which llvmkit stores as an element list.
///
/// **No upstream counterpart**, as for
/// `can_create_poison_of_a_constant_expression`: the oracle is
/// `canCreateUndefOrPoison`'s `ShuffleVector` arm, which asks only whether the
/// mask has a poison lane, and the `dyn_cast<Operator>` arm of
/// `isGuaranteedNotToBeUndefOrPoison`, which checks the shuffle's two operands
/// (`ConstantExpr::getShuffleVector` keeps the mask out of them). The second is
/// `poison`, so the splat is not proven free of poison; it is proven free of
/// `undef` when the `insertelement` is, that is, when `V` is.
#[test]
fn a_scalable_splat_is_read_as_the_shuffle_it_prints_as() {
    let rows: [((bool, bool), &str); 1] = [(
        (false, false),
        "<vscale x 4 x i32> shufflevector (<vscale x 4 x i32> insertelement (<vscale x 4 x i32> poison, i32 ptrtoint (ptr @g to i32), i64 0), <vscale x 4 x i32> poison, <vscale x 4 x i32> zeroinitializer)",
    )];
    let module = frozen_operands(&rows);
    let splat = named_instruction(&module, "r0")
        .operand(0)
        .expect("a freeze has an operand");
    assert_eq!(&splat.to_string(), rows[0].1);
    assert!(!can_create_poison(splat, true));
    assert!(!can_create_undef_or_poison(splat, true));

    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(!not_poison(splat, &query));
    assert!(!well_defined(splat, &query));
    assert!(
        llvmkit_ir::is_known_not_undef(splat, &query).expect("query succeeds"),
        "the splatted value is a cast of a global, so nothing in the splat is undef"
    );
}

/// An `insertelement` or `extractelement` of a scalable vector, at a lane
/// below its known minimum length, cannot create poison.
///
/// **No upstream counterpart.** The rows are
/// `TEST(ValueTracking, canCreatePoisonOrUndef)`'s fixed-width
/// `extractelement <4 x i32> %vx, i32 3` and `insertelement <4 x i32> %vx, i32
/// %x, i32 3` rows, asked over its scalable `%svx` instead. The oracle is the
/// arm itself, `return !Idx ||
/// Idx->getValue().uge(VTy->getElementCount().getKnownMinValue());`: a
/// scalable vector is at least its known minimum long, so a lane below it is
/// in range. The control is upstream's own pair of lane-4 rows over `%svx`,
/// which [`can_create_poison_or_undef`] ports with the rest of that table.
#[test]
fn a_scalable_lane_below_the_known_minimum_is_in_range() {
    let rows: [((bool, bool), &str); 2] = [
        (
            (false, false),
            "extractelement <vscale x 4 x i32> %svx, i32 3",
        ),
        (
            (false, false),
            "insertelement <vscale x 4 x i32> %svx, i32 %x, i32 3",
        ),
    ];
    let mut source = String::from("define void @f(<vscale x 4 x i32> %svx, i32 %x) {\n");
    for (index, (_, instruction)) in rows.iter().enumerate() {
        source.push_str(&format!("  %r{index} = {instruction}\n"));
    }
    source.push_str("  ret void\n}\n");
    let module = parse(&source);
    for (index, ((poison, undef), written)) in rows.iter().enumerate() {
        let instruction = named_value(&module, &format!("r{index}"));
        assert_eq!(
            can_create_poison(instruction, true),
            *poison,
            "canCreatePoison of row {index}: {written}"
        );
        assert_eq!(
            can_create_undef_or_poison(instruction, true),
            *undef || *poison,
            "canCreateUndefOrPoison of row {index}: {written}"
        );
    }
}

/// Builds `%x3` to `%x0` before the `ret` that opens `%body`, each an `xor` of
/// the one before with 1, the first over `@a - @b`. That symbol difference is
/// built with `try_delta_from`, llvmkit's compact form, which no `.ll` spells.
/// Returns `%x0` and the `ret`, the context instruction.
///
/// Asking about `%x0` reaches `%x3` at depth 3, the difference at depth 4, and
/// its two casts, `ptrtoint (ptr @a to i64)` and `ptrtoint (ptr @b to i64)`, at
/// depth 5. There each cast asks about its global at
/// `MaxAnalysisRecursionDepth`, which answers `false` on its first line, so the
/// casts' operand walks fail. Only the arms after the walk, the
/// dominating-condition loop among them, can prove a cast.
fn xor_chain_over_a_symbol_difference(
    module: &Module<DynBrand, Unverified>,
) -> (
    Value<'_, DynBrand>,
    InstructionView<'_, DynBrand, llvmkit_ir::ReadOnly>,
) {
    let a = module.global("a").expect("fixture defines @a");
    let b = module.global("b").expect("fixture defines @b");
    let difference = module
        .view(a)
        .try_delta_from(module.view(b))
        .expect("@a and @b are globals of one module");
    let context = module
        .as_view()
        .functions()
        .flat_map(|function| function.basic_blocks())
        .find(|block| block.name().as_deref() == Some("body"))
        .expect("fixture defines %body")
        .instructions()
        .next()
        .expect("%body is not empty");
    let unpositioned: IrBuilder<'_, '_, DynBrand, NoFolder, Unpositioned, Dyn> =
        IrBuilder::with_folder(module, NoFolder);
    let builder = unpositioned
        .position_before(context.placed().expect("the `ret` is in a block"))
        .expect("the `ret` is this module's");
    let x3 = builder
        .int_xor(difference, 1_i64, "x3")
        .expect("an `xor` of two `i64`s builds");
    let x2 = builder.int_xor(x3, 1_i64, "x2").expect("builds");
    let x1 = builder.int_xor(x2, 1_i64, "x1").expect("builds");
    let x0 = builder.int_xor(x1, 1_i64, "x0").expect("builds");
    (module.view(x0).as_erased(), context)
}

/// At the recursion limit, a dominating `switch` on a cast that a symbol
/// difference prints inside itself proves the cast, so the difference and the
/// `xor`s over it are well defined.
///
/// **The IR has no upstream counterpart.** The oracle is the
/// dominating-condition loop of `isGuaranteedNotToBeUndefOrPoison`
/// (`ValueTracking.cpp`), whose `if (Cond == V)` compares uniqued constants: a
/// condition that is the same expression is `V`. llvmkit stores the difference
/// and not its casts, so the cast is compared with each stored condition as the
/// expression it is. The `switch` is the real test of that comparison. A `br`
/// condition is an `i1` and the cast is an `i64`, so only a `switch` can be the
/// cast itself. And for the question `freeze` asks, which includes `undef`,
/// `Cond == V` is the only comparison the loop makes. The unanchored query is
/// the control: without the loop, nothing proves the casts.
#[test]
fn a_dominating_switch_proves_a_cast_a_symbol_difference_prints() {
    let module = parse(
        r"
@a = global i32 0
@b = global i32 0

define void @test() {
entry:
  switch i64 ptrtoint (ptr @a to i64), label %next []

next:
  switch i64 ptrtoint (ptr @b to i64), label %body []

body:
  ret void
}
",
    );
    let (x0, context) = xor_chain_over_a_symbol_difference(&module);
    let dominator_tree = dominator_tree(&module);
    let data_layout = module.data_layout();

    let unanchored = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(
        !well_defined(x0, &unanchored),
        "no arm but the dominating-condition loop can prove the casts"
    );

    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
        .with_dominator_tree(&dominator_tree)
        .with_context_instruction(&context);
    assert!(well_defined(x0, &query));
}

/// The poison half of the same loop: a dominating `br` on an `icmp` of the cast
/// proves the cast not poison, because the `icmp` propagates poison from its
/// operand (`propagatesPoison`'s `ICmp` arm). It does not prove the cast free
/// of `undef`: upstream looks at the condition's operands only when
/// `!includesUndef(Kind)`.
///
/// **The IR has no upstream counterpart**; the oracle is the loop's
/// `else if (!includesUndef(Kind) && isa<Operator>(Cond))` arm. The unanchored
/// query is the control, as above.
#[test]
fn a_dominating_branch_on_a_compare_of_the_cast_proves_it_not_poison() {
    let module = parse(
        r"
@a = global i32 0
@b = global i32 0

define void @test() {
entry:
  %ca = icmp eq i64 ptrtoint (ptr @a to i64), 0
  br i1 %ca, label %next, label %exit

next:
  %cb = icmp eq i64 ptrtoint (ptr @b to i64), 0
  br i1 %cb, label %body, label %exit

body:
  ret void

exit:
  ret void
}
",
    );
    let (x0, context) = xor_chain_over_a_symbol_difference(&module);
    let dominator_tree = dominator_tree(&module);
    let data_layout = module.data_layout();

    let unanchored = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(
        !not_poison(x0, &unanchored),
        "no arm but the dominating-condition loop can prove the casts"
    );

    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
        .with_dominator_tree(&dominator_tree)
        .with_context_instruction(&context);
    assert!(not_poison(x0, &query));
    assert!(
        !well_defined(x0, &query),
        "a `br` reaches the cast only through the poison-only arm"
    );
}

/// `TEST(ValueTracking, canCreatePoisonOrUndef)` from
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`.
///
/// The assembly head and tail, the rows of `(can create poison?, can create
/// undef?, IR instruction)` and the two `EXPECT_EQ`s are upstream's, verbatim
/// and in upstream's order. The rows are joined into one function body the way
/// upstream's loop joins them, each instruction unnamed, and the entry block is
/// walked up to its `ret`, the `Index`-th instruction checked against the
/// `Index`-th row. `canCreatePoison` and `canCreateUndefOrPoison` are called
/// as upstream calls them, with the default `ConsiderFlagsAndMetadata = true`.
/// `EXPECT_EQ` does not stop at a failure, so every mismatch is collected and
/// the test fails once, naming them all. Each message is upstream's prefix
/// ("Incorrect answer of canCreatePoison at instruction N = "); where upstream
/// streams the printed instruction after it, the row's own IR text stands in.
///
/// One assertion is llvmkit's: the walk must reach as many instructions as
/// there are rows. Upstream needs no such check, because its parser keeps each
/// row as one instruction; here it guards against a row the parser folds or
/// drops, which would otherwise shift every later row onto the wrong answer.
#[test]
fn can_create_poison_or_undef() {
    let asm_head = concat!(
        "@s = external dso_local global i32, align 1\n",
        "declare i32 @g(i32)\n",
        "declare {i32, i1} @llvm.sadd.with.overflow.i32(i32 %a, i32 %b)\n",
        "declare {i32, i1} @llvm.ssub.with.overflow.i32(i32 %a, i32 %b)\n",
        "declare {i32, i1} @llvm.smul.with.overflow.i32(i32 %a, i32 %b)\n",
        "declare {i32, i1} @llvm.uadd.with.overflow.i32(i32 %a, i32 %b)\n",
        "declare {i32, i1} @llvm.usub.with.overflow.i32(i32 %a, i32 %b)\n",
        "declare {i32, i1} @llvm.umul.with.overflow.i32(i32 %a, i32 %b)\n",
        "define void @f(i32 %x, i32 %y, float %fx, float %fy, i1 %cond, ",
        "<4 x i32> %vx, <4 x i32> %vx2, <vscale x 4 x i32> %svx, ptr %p) {\n",
    );
    let asm_tail = "  ret void\n}";
    // (can create poison?, can create undef?, IR instruction)
    let data: &[((bool, bool), &str)] = &[
        ((false, false), "add i32 %x, %y"),
        ((true, false), "add nsw nuw i32 %x, %y"),
        ((true, false), "shl i32 %x, %y"),
        ((true, false), "shl <4 x i32> %vx, %vx2"),
        ((true, false), "shl nsw i32 %x, %y"),
        (
            (true, false),
            "shl nsw <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 3>",
        ),
        ((false, false), "shl i32 %x, 31"),
        ((true, false), "shl i32 %x, 32"),
        (
            (false, false),
            "shl <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 3>",
        ),
        (
            (true, false),
            "shl <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 32>",
        ),
        ((true, false), "ashr i32 %x, %y"),
        ((true, false), "ashr exact i32 %x, %y"),
        ((false, false), "ashr i32 %x, 31"),
        ((true, false), "ashr exact i32 %x, 31"),
        (
            (false, false),
            "ashr <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 3>",
        ),
        (
            (true, false),
            "ashr <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 32>",
        ),
        (
            (true, false),
            "ashr exact <4 x i32> %vx, <i32 0, i32 1, i32 2, i32 3>",
        ),
        ((true, false), "lshr i32 %x, %y"),
        ((true, false), "lshr exact i32 %x, 31"),
        ((false, false), "udiv i32 %x, %y"),
        ((true, false), "udiv exact i32 %x, %y"),
        ((false, false), "getelementptr i8, ptr %p, i32 %x"),
        ((true, false), "getelementptr inbounds i8, ptr %p, i32 %x"),
        ((true, false), "fneg nnan float %fx"),
        ((false, false), "fneg float %fx"),
        ((false, false), "fadd float %fx, %fy"),
        ((true, false), "fadd nnan float %fx, %fy"),
        ((false, false), "urem i32 %x, %y"),
        ((true, false), "fptoui float %fx to i32"),
        ((true, false), "fptosi float %fx to i32"),
        ((false, false), "bitcast float %fx to i32"),
        ((false, false), "select i1 %cond, i32 %x, i32 %y"),
        ((true, false), "select nnan i1 %cond, float %fx, float %fy"),
        ((true, false), "extractelement <4 x i32> %vx, i32 %x"),
        ((false, false), "extractelement <4 x i32> %vx, i32 3"),
        (
            (true, false),
            "extractelement <vscale x 4 x i32> %svx, i32 4",
        ),
        ((true, false), "insertelement <4 x i32> %vx, i32 %x, i32 %y"),
        ((false, false), "insertelement <4 x i32> %vx, i32 %x, i32 3"),
        (
            (true, false),
            "insertelement <vscale x 4 x i32> %svx, i32 %x, i32 4",
        ),
        ((false, false), "freeze i32 %x"),
        (
            (false, false),
            concat!(
                "shufflevector <4 x i32> %vx, <4 x i32> %vx2, ",
                "<4 x i32> <i32 0, i32 1, i32 2, i32 3>"
            ),
        ),
        (
            (true, false),
            concat!(
                "shufflevector <4 x i32> %vx, <4 x i32> %vx2, ",
                "<4 x i32> <i32 0, i32 1, i32 2, i32 poison>"
            ),
        ),
        (
            (true, false),
            concat!(
                "shufflevector <vscale x 4 x i32> %svx, ",
                "<vscale x 4 x i32> %svx, <vscale x 4 x i32> poison"
            ),
        ),
        ((true, false), "call i32 @g(i32 %x)"),
        ((false, false), "call noundef i32 @g(i32 %x)"),
        ((true, false), "fcmp nnan oeq float %fx, %fy"),
        ((false, false), "fcmp oeq float %fx, %fy"),
        ((true, false), "ashr i32 %x, ptrtoint (ptr @s to i32)"),
        (
            (false, false),
            "call {i32, i1} @llvm.sadd.with.overflow.i32(i32 %x, i32 %y)",
        ),
        (
            (false, false),
            "call {i32, i1} @llvm.ssub.with.overflow.i32(i32 %x, i32 %y)",
        ),
        (
            (false, false),
            "call {i32, i1} @llvm.smul.with.overflow.i32(i32 %x, i32 %y)",
        ),
        (
            (false, false),
            "call {i32, i1} @llvm.uadd.with.overflow.i32(i32 %x, i32 %y)",
        ),
        (
            (false, false),
            "call {i32, i1} @llvm.usub.with.overflow.i32(i32 %x, i32 %y)",
        ),
        (
            (false, false),
            "call {i32, i1} @llvm.umul.with.overflow.i32(i32 %x, i32 %y)",
        ),
    ];

    let mut assembly_str = String::from(asm_head);
    for (_, instruction) in data {
        assembly_str.push_str(instruction);
        assembly_str.push('\n');
    }
    assembly_str.push_str(asm_tail);

    let module = parse(&assembly_str);
    let function = module.function_dyn("f").expect("Bad assembly?");
    let block = module
        .view(function)
        .basic_blocks()
        .next()
        .expect("Bad assembly?");

    let mut mismatches = Vec::new();
    let mut index = 0;
    for instruction in block.instructions() {
        if matches!(instruction.terminator_kind(), Some(TerminatorKind::Ret(_))) {
            break;
        }
        let ((poison, undef), written) = data[index];
        let operator = instruction.to_erased();
        if can_create_poison(operator, true) != poison {
            mismatches.push(format!(
                "Incorrect answer of canCreatePoison at instruction {index} = {written}"
            ));
        }
        if can_create_undef_or_poison(operator, true) != (undef || poison) {
            mismatches.push(format!(
                "Incorrect answer of canCreateUndef at instruction {index} = {written}"
            ));
        }
        index += 1;
    }
    assert_eq!(index, data.len(), "each row is one instruction of @f");
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// `canCreatePoison` and `canCreateUndefOrPoison` of the instructions whose
/// annotations, intrinsic cases and shift amounts
/// `TEST(ValueTracking, canCreatePoisonOrUndef)` has no row for.
///
/// **No upstream counterpart.** The rows are in that test's shape, `(can
/// create poison?, can create undef?, IR)`, each named `%rN` here so that a row
/// the parser folds fails by name. The oracles are the arms each row reaches:
///
/// - `Operator::hasPoisonGeneratingFlags` (`Operator.cpp`): `trunc`'s `nuw` and
///   `nsw`; `nneg` of `zext` and `uitofp`; `disjoint`; a `getelementptr`'s
///   `nuw` and `nusw`; `samesign`; and, in its `default:` arm, `nnan` or `ninf`
///   of an `FPMathOperator`, which `fpext`, `fptrunc`, `fsub`, and a `select`,
///   a `call` and a `phi` of floating-point type are, while `nsz` is no
///   poison-generating flag. The two `phi`s open the block the rows are in.
/// - `Instruction::hasPoisonGeneratingReturnAttributes` (`Instruction.cpp`):
///   `range`, `align` or `nonnull` on the call site's own return, and not
///   `dereferenceable`, nor a `range` that only the declaration carries.
/// - `Instruction::hasPoisonGeneratingMetadata`: `!range`, `!nonnull` and
///   `!align` (`Metadata::PoisonGeneratingIDs`), and not `!noundef`.
/// - `canCreateUndefOrPoison`'s intrinsic `switch`: `ctlz`, `cttz` and `abs`
///   with a null second operand cannot create poison; `sshl.sat` and
///   `ushl.sat` cannot when the shift amount is in range.
/// - Its `CallBase` arm, `!CB->hasRetAttr(Attribute::NoUndef)`, which reads the
///   declaration's `noundef` when `getCalledFunction()` names the callee, and
///   not when the call's function type is another.
/// - `shiftAmountKnownInRange` over a fixed vector's lanes, each read by
///   `getAggregateElement`: a zero vector and a splat are in range, a `poison`
///   lane is no `ConstantInt`, and a scalable vector cannot be told.
///
/// For each kind of annotation the rows carry, at least one row that the
/// annotation alone makes poison is asked again with `ConsiderFlagsAndMetadata`
/// off, where it answers as its opcode does.
#[test]
fn can_create_poison_reads_every_annotation_and_arm() {
    let rows: &[((bool, bool), &str)] = &[
        ((true, false), "trunc nuw i32 %x to i8"),
        ((true, false), "trunc nsw i32 %x to i8"),
        ((false, false), "trunc i32 %x to i8"),
        ((true, false), "zext nneg i8 %b to i32"),
        ((true, false), "uitofp nneg i32 %x to float"),
        ((false, false), "uitofp i32 %x to float"),
        ((true, false), "or disjoint i32 %x, %y"),
        ((true, false), "getelementptr nuw i8, ptr %p, i32 %x"),
        ((true, false), "getelementptr nusw i8, ptr %p, i32 %x"),
        ((true, false), "icmp samesign ult i32 %x, %y"),
        ((false, false), "icmp ult i32 %x, %y"),
        ((true, false), "fpext nnan float %fx to double"),
        ((true, false), "fptrunc ninf double %d to float"),
        ((false, false), "fpext float %fx to double"),
        ((true, false), "fsub ninf float %fx, %fy"),
        ((false, false), "fsub nsz float %fx, %fy"),
        ((true, false), "select ninf i1 %cond, float %fx, float %fy"),
        ((true, false), "call nnan noundef float @fp(float %fx)"),
        ((false, false), "call nsz noundef float @fp(float %fx)"),
        (
            (true, false),
            "call noundef range(i32 0, 10) i32 @g(i32 %x)",
        ),
        ((true, false), "call noundef nonnull ptr @gp()"),
        ((true, false), "call noundef align 8 ptr @gp()"),
        ((false, false), "call noundef dereferenceable(4) ptr @gp()"),
        ((false, false), "call noundef i32 @gr(i32 %x)"),
        ((true, false), "call noundef i32 @g(i32 %x), !range !0"),
        ((true, false), "call noundef ptr @gp(), !nonnull !1"),
        ((true, false), "call noundef ptr @gp(), !align !2"),
        ((false, false), "call noundef ptr @gp(), !noundef !1"),
        ((false, false), "call i32 @llvm.ctlz.i32(i32 %x, i1 false)"),
        ((true, false), "call i32 @llvm.ctlz.i32(i32 %x, i1 true)"),
        ((false, false), "call i32 @llvm.cttz.i32(i32 %x, i1 false)"),
        ((false, false), "call i32 @llvm.abs.i32(i32 %x, i1 false)"),
        ((true, false), "call i32 @llvm.abs.i32(i32 %x, i1 true)"),
        (
            (false, false),
            "call i32 @llvm.sshl.sat.i32(i32 %x, i32 31)",
        ),
        ((true, false), "call i32 @llvm.sshl.sat.i32(i32 %x, i32 32)"),
        ((true, false), "call i32 @llvm.ushl.sat.i32(i32 %x, i32 %y)"),
        (
            (false, false),
            "call i32 @llvm.ushl.sat.i32(i32 %x, i32 31)",
        ),
        ((false, false), "call i32 @h(i32 %x)"),
        ((true, false), "call i64 @h(i32 %x)"),
        ((false, false), "shl <4 x i32> %vx, zeroinitializer"),
        ((false, false), "shl <4 x i32> %vx, splat (i32 1)"),
        (
            (true, false),
            "shl <4 x i32> %vx, <i32 0, i32 poison, i32 2, i32 3>",
        ),
        ((true, false), "shl <vscale x 4 x i32> %svx, splat (i32 1)"),
    ];
    let mut source = String::from(concat!(
        "declare i32 @g(i32)\n",
        "declare noundef i32 @h(i32)\n",
        "declare range(i32 0, 10) i32 @gr(i32)\n",
        "declare ptr @gp()\n",
        "declare float @fp(float)\n",
        "declare i32 @llvm.ctlz.i32(i32, i1)\n",
        "declare i32 @llvm.cttz.i32(i32, i1)\n",
        "declare i32 @llvm.abs.i32(i32, i1)\n",
        "declare i32 @llvm.sshl.sat.i32(i32, i32)\n",
        "declare i32 @llvm.ushl.sat.i32(i32, i32)\n",
        "\n",
        "define void @f(i32 %x, i32 %y, i8 %b, float %fx, float %fy, double %d, i1 %cond, ",
        "<4 x i32> %vx, <vscale x 4 x i32> %svx, ptr %p) {\n",
        "entry:\n",
        "  br label %body\n",
        "\n",
        "body:\n",
        "  %phi.nnan = phi nnan float [ %fx, %entry ]\n",
        "  %phi.plain = phi float [ %fx, %entry ]\n",
    ));
    for (index, (_, instruction)) in rows.iter().enumerate() {
        source.push_str(&format!("  %r{index} = {instruction}\n"));
    }
    source.push_str("  ret void\n}\n\n!0 = !{i32 0, i32 10}\n!1 = !{}\n!2 = !{i64 8}\n");
    let module = parse(&source);

    let mut mismatches = Vec::new();
    for (index, ((poison, undef), written)) in rows.iter().enumerate() {
        let instruction = named_value(&module, &format!("r{index}"));
        if can_create_poison(instruction, true) != *poison {
            mismatches.push(format!("canCreatePoison of row {index}: {written}"));
        }
        if can_create_undef_or_poison(instruction, true) != (*undef || *poison) {
            mismatches.push(format!("canCreateUndefOrPoison of row {index}: {written}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));

    // A `phi` of floating-point type is an `FPMathOperator` through the same
    // `classof` arm as a `select` and a `call`, so `nnan` makes it poison-
    // generating; without the flag, the switch's `case Instruction::PHI:`
    // answers that it creates nothing.
    let phi_nnan = named_value(&module, "phi.nnan");
    assert!(can_create_poison(phi_nnan, true), "phi nnan float");
    assert!(can_create_undef_or_poison(phi_nnan, true), "phi nnan float");
    assert!(
        !can_create_poison(phi_nnan, false),
        "phi nnan float, no flags"
    );
    let phi_plain = named_value(&module, "phi.plain");
    assert!(!can_create_poison(phi_plain, true), "phi float");
    assert!(!can_create_undef_or_poison(phi_plain, true), "phi float");

    // `ConsiderFlagsAndMetadata` gates the annotations: without it, each of
    // these answers as its opcode does. A cast, an `or`, a `getelementptr`, an
    // `icmp` and a `select` cannot create poison, and neither can a `call
    // noundef`, because the `CallBase` arm answers `false` once
    // `hasRetAttr(NoUndef)` holds.
    for index in [1, 3, 6, 7, 9, 11, 16, 17, 19, 24] {
        let instruction = named_value(&module, &format!("r{index}"));
        assert!(
            !can_create_poison(instruction, false),
            "row {index} without its annotations: {}",
            rows[index].1
        );
    }
}

/// An `invoke` and a `callbr` answer `canCreatePoison` through the same
/// `CallBase` reads a `call` does.
///
/// **No upstream counterpart**: `TEST(ValueTracking, canCreatePoisonOrUndef)`
/// asks only `call`s. The oracles are `Instruction::hasPoisonGeneratingReturnAttributes`
/// (`Instruction.cpp`), whose `dyn_cast<CallBase>` admits both, and
/// `canCreateUndefOrPoison`'s `case Instruction::CallBr: case
/// Instruction::Invoke:` arm, `!CB->hasRetAttr(Attribute::NoUndef) &&
/// !CB->hasFnAttr(Attribute::NoCreateUndefOrPoison)`, with `hasRetAttr`'s
/// `getCalledFunction()` fallback. A `range` return attribute makes each
/// poison-generating, a `noundef` one, written or declared, makes each unable
/// to create poison, and the plain one of each kind is the control.
#[test]
fn the_invoke_and_callbr_arms_read_the_same_attributes_as_a_call() {
    let module = parse(
        r"
declare i32 @g(i32)
declare noundef i32 @h(i32)
declare i32 @personality(...)

define void @test(i32 %x) personality ptr @personality {
entry:
  %invoke.range = invoke noundef range(i32 0, 10) i32 @g(i32 %x) to label %b1 unwind label %lp

b1:
  %invoke.noundef = invoke noundef i32 @g(i32 %x) to label %b2 unwind label %lp

b2:
  %invoke.declared = invoke i32 @h(i32 %x) to label %b3 unwind label %lp

b3:
  %invoke.plain = invoke i32 @g(i32 %x) to label %b4 unwind label %lp

b4:
  %callbr.range = callbr noundef range(i32 0, 10) i32 @g(i32 %x) to label %b5 []

b5:
  %callbr.noundef = callbr noundef i32 @g(i32 %x) to label %b6 []

b6:
  %callbr.declared = callbr i32 @h(i32 %x) to label %b7 []

b7:
  %callbr.plain = callbr i32 @g(i32 %x) to label %b8 []

b8:
  ret void

lp:
  %pad = landingpad { ptr, i32 } cleanup
  ret void
}
",
    );
    for kind in ["invoke", "callbr"] {
        let value = |suffix: &str| named_value(&module, &format!("{kind}.{suffix}"));
        assert!(
            can_create_poison(value("range"), true),
            "{kind} noundef range: the range return attribute is poison-generating"
        );
        assert!(
            !can_create_poison(value("range"), false),
            "{kind} noundef range, no annotations: noundef settles the CallBase arm"
        );
        assert!(!can_create_poison(value("noundef"), true), "{kind} noundef");
        assert!(
            !can_create_poison(value("declared"), true),
            "{kind} of a function declared noundef"
        );
        assert!(can_create_poison(value("plain"), true), "{kind} plain");
        assert!(
            can_create_undef_or_poison(value("plain"), true),
            "{kind} plain"
        );
    }
}

/// The two `CallBase` arms that read `CallBase::hasRetAttr`, which looks at the
/// called function's attributes when the call site has none, and the intrinsic
/// `switch`'s `!includesPoison(Kind)` test.
///
/// **No upstream counterpart.** The oracles are
/// `isGuaranteedNotToBeUndefOrPoison`'s early `dyn_cast<CallBase>` arm,
/// `CB->hasRetAttr(Attribute::NoUndef) || CB->hasRetAttr(Attribute::Dereferenceable)
/// || CB->hasRetAttr(Attribute::DereferenceableOrNull)`, each with
/// `hasRetAttrImpl`'s `if (const Function *F = getCalledFunction())` fallback
/// (`InstrTypes.h`), and `canCreateUndefOrPoison`'s `case Intrinsic::ushl_sat:
/// if (!includesPoison(Kind) || ...) return false;`. Which assertion pins which
/// arm:
///
/// - `%declared` and `%deref` / `%deref.or.null` are proven by the early arm
///   alone. `%declared`'s operand `%z` carries no `noundef`, so the later
///   `canCreateUndefOrPoison` arm's operand walk fails on it, and `@d` / `@dn`
///   declare no `noundef`, which is all that later arm reads. Without the early
///   arm's callee fallback each would answer no.
/// - `%declared.noundef.operand` is proven either way: its operand is `noundef`,
///   so the later arm's walk proves it too. It is the case that cannot tell the
///   two arms apart.
/// - `%undeclared`, a call of `@g`, declared without any of the three, is the
///   control.
///
/// A `ushl.sat` over `noundef` operands by an amount that may be out of range
/// can be poison, but it cannot create `undef`, so it is known not `undef` and
/// not known not poison.
#[test]
fn a_call_reads_its_callee_s_noundef_and_a_saturating_shift_creates_no_undef() {
    let module = parse(
        r"
declare noundef i32 @h(i32)
declare i32 @g(i32)
declare dereferenceable(4) ptr @d()
declare dereferenceable_or_null(4) ptr @dn()
declare i32 @llvm.ushl.sat.i32(i32, i32)

define void @f(i32 noundef %x, i32 noundef %y, i32 %z) {
  %declared = call i32 @h(i32 %z)
  %declared.noundef.operand = call i32 @h(i32 %x)
  %deref = call ptr @d()
  %deref.or.null = call ptr @dn()
  %undeclared = call i32 @g(i32 %x)
  %shifted = call i32 @llvm.ushl.sat.i32(i32 %x, i32 %y)
  ret void
}
",
    );
    let data_layout = module.data_layout();
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout);
    assert!(
        well_defined(named_value(&module, "declared"), &query),
        "the early arm reads @h's noundef; the operand walk fails on %z"
    );
    assert!(well_defined(
        named_value(&module, "declared.noundef.operand"),
        &query
    ));
    assert!(
        well_defined(named_value(&module, "deref"), &query),
        "the early arm reads @d's dereferenceable"
    );
    assert!(
        well_defined(named_value(&module, "deref.or.null"), &query),
        "the early arm reads @dn's dereferenceable_or_null"
    );
    assert!(!well_defined(named_value(&module, "undeclared"), &query));

    let shifted = named_value(&module, "shifted");
    assert!(
        llvmkit_ir::is_known_not_undef(shifted, &query).expect("query succeeds"),
        "a saturating shift cannot create undef, and its operands are noundef"
    );
    assert!(!not_poison(shifted, &query));
}

/// A `speculatable` call with a UB-implying attribute on any argument or on its
/// return, written on the call site or declared on the callee, is not safe to
/// speculate once those attributes are honoured.
///
/// **No upstream counterpart**: no upstream unit test calls
/// `isSafeToSpeculativelyExecute` with `IgnoreUBImplyingAttrs = false`. The
/// oracle is its `Call` arm's `return IgnoreUBImplyingAttrs ||
/// !CI->hasUBImplyingAttrs();` with `Instruction::hasUBImplyingAttrs`
/// (`Instruction.cpp`): `CB->isPassingUndefUB(ArgNo)` for every argument, then
/// `CB->hasRetAttr` of `noundef`, `dereferenceable` and
/// `dereferenceable_or_null`. `isPassingUndefUB` is three
/// `CallBase::paramHasAttr` calls (`Instructions.cpp`) and `hasRetAttr` is
/// `hasRetAttrImpl`; each reads the call site and then `getCalledFunction()`.
///
/// `%plain` is the control. So is the default options' answer, which ignores the
/// attributes, as upstream's defaulted `IgnoreUBImplyingAttrs = true` does.
/// `%first` is the one row that can be read off argument 0 alone. `%second`
/// carries `noundef` on argument 1, which is filed under that argument's own
/// attribute set like every call-site argument attribute.
#[test]
fn a_speculatable_call_with_a_ub_implying_attribute_anywhere_is_not_safe_to_speculate() {
    use llvmkit_ir::{SpeculationOptions, is_safe_to_speculatively_execute};

    let module = parse(
        r"
declare i32 @f(i32, i32) speculatable
declare i32 @second.declared(i32, i32 noundef) speculatable
declare i32 @second.dereferenceable(i32, ptr dereferenceable(4)) speculatable
declare noundef i32 @return.declared(i32) speculatable
declare dereferenceable(4) ptr @return.dereferenceable() speculatable
declare dereferenceable_or_null(4) ptr @return.dereferenceable.or.null() speculatable

define void @test(i32 %a, i32 %b, ptr %p) {
  %plain = call i32 @f(i32 %a, i32 %b)
  %first = call i32 @f(i32 noundef %a, i32 %b)
  %second = call i32 @f(i32 %a, i32 noundef %b)
  %second.declared = call i32 @second.declared(i32 %a, i32 %b)
  %second.dereferenceable = call i32 @second.dereferenceable(i32 %a, ptr %p)
  %return = call noundef i32 @f(i32 %a, i32 %b)
  %return.declared = call i32 @return.declared(i32 %a)
  %return.dereferenceable = call ptr @return.dereferenceable()
  %return.dereferenceable.or.null = call ptr @return.dereferenceable.or.null()
  ret void
}
",
    );
    let honouring = SpeculationOptions::new().honouring_ub_implying_attrs();
    let ignoring = SpeculationOptions::default();
    let call = |name: &str| named_instruction(&module, name);

    assert!(is_safe_to_speculatively_execute(&call("plain"), honouring));
    for name in [
        "first",
        "second",
        "second.declared",
        "second.dereferenceable",
        "return",
        "return.declared",
        "return.dereferenceable",
        "return.dereferenceable.or.null",
    ] {
        assert!(
            !is_safe_to_speculatively_execute(&call(name), honouring),
            "%{name} carries a UB-implying attribute"
        );
        assert!(
            is_safe_to_speculatively_execute(&call(name), ignoring),
            "%{name}: the default options ignore UB-implying attributes"
        );
    }
}

/// Poison in an argument that carries `noundef`, on the call site or on the
/// callee's declaration, is undefined behaviour at the call, whichever argument
/// it is.
///
/// **No upstream counterpart**: `TEST_F(ValueTrackingTest,
/// programUndefinedIfPoison)` reaches its UB through a `udiv`. The oracle is
/// `handleGuaranteedWellDefinedOps`'s `case Instruction::Call:` arm
/// (`ValueTracking.cpp`), `CB->paramHasAttr(i, Attribute::NoUndef) ||
/// CB->paramHasAttr(i, Attribute::Dereferenceable) || CB->paramHasAttr(i,
/// Attribute::DereferenceableOrNull)` for every argument `i`, where
/// `paramHasAttr` reads the call site and then `getCalledFunction()`'s
/// declaration. `%c` is the control: it is passed only in an argument with no
/// such attribute. Each call sits in its own function, because the scan stops
/// at a call that is not guaranteed to transfer execution to its successor,
/// which a call of a function declared without `nounwind willreturn` is not.
#[test]
fn poison_in_a_noundef_argument_after_the_first_is_undefined_behaviour() {
    let module = parse(
        r"
declare void @f(i32, i32)
declare void @g(i32, i32 noundef)

define void @call_site(i32 %x) {
  %a = add i32 %x, 1
  %c = add i32 %x, 3
  call void @f(i32 %c, i32 noundef %a)
  ret void
}

define void @declared(i32 %x) {
  %b = add i32 %x, 2
  call void @g(i32 %x, i32 %b)
  ret void
}
",
    );
    assert!(program_undefined_if_poison(&named_instruction(
        &module, "a"
    )));
    assert!(program_undefined_if_poison(&named_instruction(
        &module, "b"
    )));
    assert!(!program_undefined_if_poison(&named_instruction(
        &module, "c"
    )));
}

/// The instruction named `%name` in the function `@function`.
fn instruction_in<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> InstructionView<'m, DynBrand> {
    let function = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    module
        .view(function)
        .basic_blocks()
        .flat_map(|block| block.instructions())
        .find(|instruction| instruction.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The question `simplifyFreezeInst` (`InstructionSimplify.cpp`) asks of the
/// `freeze` named `%freeze` in `@function`:
/// `isGuaranteedNotToBeUndefOrPoison(Op0, Q.AC, Q.CxtI, Q.DT)`, with the
/// `freeze` as the context instruction and the function's dominator tree. A
/// yes is the answer under which `instsimplify` replaces the `freeze` by its
/// operand.
fn freeze_simplifies_to_its_operand(
    module: &Module<DynBrand, Unverified>,
    function: &str,
    freeze: &str,
) -> bool {
    let id = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let dominator_tree = DominatorTree::new(module.view(id));
    let data_layout = module.data_layout();
    let freeze = instruction_in(module, function, freeze);
    let operand = freeze.operand(0).expect("a freeze has an operand");
    let query = ValueTrackingQuery::<DynBrand>::new(&data_layout)
        .with_dominator_tree(&dominator_tree)
        .with_context_instruction(&freeze);
    is_known_not_undef_or_poison(operand, &query).expect("query succeeds")
}

/// Ports `@icmp`, `@icmp_noopt`, `@fcmp` and `@fcmp_noopt` from
/// `llvm/test/Transforms/InstSimplify/freeze.ll`, IR unchanged. Each is asked
/// the question `simplifyFreezeInst` asks of its frozen compare
/// ([`freeze_simplifies_to_its_operand`]). A `CHECK` block that returns the
/// compare expects yes, and one that keeps the `freeze` expects no.
///
/// They are asked at the routine because `InstSimplifyPass` does not ask it:
/// the pass reaches a `freeze` only through `constant_fold_instruction`, whose
/// `case Instruction::Freeze:` takes a constant operand, so a `freeze` of an
/// instruction never folds there (`docs/future-work.md`, "InstSimplify freeze
/// folds").
///
/// `@fcmp_noopt` keeps its `freeze` because `nnan` is a poison-generating
/// flag: `Operator::hasPoisonGeneratingFlags`'s `default:` arm asks the
/// `FPMathOperator` for `hasNoNaNs() || hasNoInfs()`, so
/// `canCreateUndefOrPoison` answers yes before it reaches the operands.
/// `@icmp_noopt` keeps its `freeze` for the other reason: its operands are
/// arguments that carry no `noundef`.
#[test]
fn inst_simplify_freeze_of_a_compare_is_the_compare_unless_it_can_create_poison() {
    let module = parse(
        r"
define i1 @icmp(i32 %a, i32 %b) {
  %a.fr = freeze i32 %a
  %b.fr = freeze i32 %b
  %c = icmp eq i32 %a.fr, %b.fr
  %c.fr = freeze i1 %c
  ret i1 %c.fr
}

define i1 @icmp_noopt(i32 %a, i32 %b) {
  %c = icmp eq i32 %a, %b
  %c.fr = freeze i1 %c
  ret i1 %c.fr
}

define i1 @fcmp(float %x, float %y) {
  %fx = freeze float %x
  %fy = freeze float %y
  %c = fcmp oeq float %fx, %fy
  %fc = freeze i1 %c
  ret i1 %fc
}

define i1 @fcmp_noopt(float %x, float %y) {
  %fx = freeze float %x
  %fy = freeze float %y
  %c = fcmp nnan oeq float %fx, %fy
  %fc = freeze i1 %c
  ret i1 %fc
}
",
    );
    assert!(freeze_simplifies_to_its_operand(&module, "icmp", "c.fr"));
    assert!(!freeze_simplifies_to_its_operand(
        &module,
        "icmp_noopt",
        "c.fr"
    ));
    assert!(freeze_simplifies_to_its_operand(&module, "fcmp", "fc"));
    assert!(!freeze_simplifies_to_its_operand(
        &module,
        "fcmp_noopt",
        "fc"
    ));
}

/// A `freeze` of an `icmp samesign` over well-defined operands is not its
/// operand, because `samesign` can make the compare poison.
///
/// **No upstream counterpart.** `test/Transforms/InstSimplify/freeze.ll` has no
/// `samesign` compare. `@icmp_samesign` is that file's `@icmp` with the flag
/// added and nothing else changed, asked the same question
/// ([`freeze_simplifies_to_its_operand`]). The oracle is
/// `Operator::hasPoisonGeneratingFlags`'s `case Instruction::ICmp: return
/// cast<ICmpInst>(this)->hasSameSign();` (`Operator.cpp`), which makes
/// `canCreateUndefOrPoison` answer yes before it reaches the operands. `@icmp`,
/// unchanged, is the positive control: the same compare without the flag is
/// well defined.
#[test]
fn a_freeze_of_a_samesign_compare_is_not_its_operand() {
    let module = parse(
        r"
define i1 @icmp(i32 %a, i32 %b) {
  %a.fr = freeze i32 %a
  %b.fr = freeze i32 %b
  %c = icmp eq i32 %a.fr, %b.fr
  %c.fr = freeze i1 %c
  ret i1 %c.fr
}

define i1 @icmp_samesign(i32 %a, i32 %b) {
  %a.fr = freeze i32 %a
  %b.fr = freeze i32 %b
  %c = icmp samesign eq i32 %a.fr, %b.fr
  %c.fr = freeze i1 %c
  ret i1 %c.fr
}
",
    );
    assert!(freeze_simplifies_to_its_operand(&module, "icmp", "c.fr"));
    assert!(!freeze_simplifies_to_its_operand(
        &module,
        "icmp_samesign",
        "c.fr"
    ));
}
