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
//!
//! A test with no upstream counterpart says so in its own doc comment.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    Capability, DominatorTree, Dyn, DynBrand, InstructionView, IrBuilder, Module, NoFolder,
    Unpositioned, Unverified, User, Value, ValueTrackingQuery, VectorType, can_create_poison,
    can_create_undef_or_poison, is_guaranteed_to_transfer_execution_to_successor,
    is_known_not_poison, is_known_not_undef_or_poison, lanes, program_undefined_if_poison,
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
/// The two rows at lane 4 are upstream's, from
/// `TEST(ValueTracking, canCreatePoisonOrUndef)` in
/// `llvm/unittests/Analysis/ValueTrackingTest.cpp`, over its `%svx`. **The
/// rows at lane 3 have no upstream counterpart**: they are its fixed-width
/// `extractelement <4 x i32> %vx, i32 3` and `insertelement <4 x i32> %vx, i32
/// %x, i32 3` rows over `%svx`. The oracle is the arm itself, `return !Idx ||
/// Idx->getValue().uge(VTy->getElementCount().getKnownMinValue());`: a
/// scalable vector is at least its known minimum long, so a lane below it is
/// in range.
#[test]
fn a_scalable_lane_below_the_known_minimum_is_in_range() {
    let rows: [((bool, bool), &str); 4] = [
        (
            (false, false),
            "extractelement <vscale x 4 x i32> %svx, i32 3",
        ),
        (
            (true, false),
            "extractelement <vscale x 4 x i32> %svx, i32 4",
        ),
        (
            (false, false),
            "insertelement <vscale x 4 x i32> %svx, i32 %x, i32 3",
        ),
        (
            (true, false),
            "insertelement <vscale x 4 x i32> %svx, i32 %x, i32 4",
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
