//! `BasicBlock::split_at` and `BasicBlock::split_before`, ported from
//! `BasicBlock::splitBasicBlock` and `BasicBlock::splitBasicBlockBefore`
//! (`llvm/lib/IR/BasicBlock.cpp`).
//!
//! The fixtures are parsed rather than built, because every upstream test of
//! these routines parses its IR with `parseAssemblyString`, and `llvmkit-ir`
//! cannot depend on the parser crate.

use llvmkit_asmparser::parser;
use llvmkit_ir::{
    BasicBlock, BlockId, BlockTerminationState, DominatorTree, Dyn, DynBrand, FunctionCfg,
    InstructionKind, InstructionView, IrError, MetadataAttachmentKind, MetadataFieldValue,
    MetadataKind, Module, Terminated, Unverified,
};

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}\n--- source ---\n{source}"),
    }
}

/// Port of `getBasicBlockByName` (`unittests/Transforms/Utils/BasicBlockUtilsTest.cpp`):
/// the block of `function` named `name`.
fn block_by_name<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    name: &str,
) -> BasicBlock<'m, Dyn, llvmkit_ir::Terminated, DynBrand> {
    let function = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    module
        .view(function)
        .basic_blocks()
        .find(|block| block.name().as_deref() == Some(name))
        .unwrap_or_else(|| panic!("fixture defines %{name}"))
}

/// The block of `function` whose id is `id`.
fn block_by_id<'m>(
    module: &'m Module<DynBrand, Unverified>,
    function: &str,
    id: BlockId<Dyn, DynBrand>,
) -> BasicBlock<'m, Dyn, Terminated, DynBrand> {
    let function_id = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    module
        .view(function_id)
        .basic_blocks()
        .find(|block| block.id() == id)
        .unwrap_or_else(|| panic!("@{function} holds the block"))
}

/// `BasicBlock::getSingleSuccessor`: the successor when the terminator has
/// exactly one successor edge, duplicate edges counted.
fn single_successor<S: BlockTerminationState>(
    block: &BasicBlock<'_, Dyn, S, DynBrand>,
) -> Option<BlockId<Dyn, DynBrand>> {
    let mut successors = block.successors();
    let first = successors.next()?;
    successors.next().is_none().then_some(first)
}

/// `BasicBlock::getSinglePredecessor`: the predecessor when the block has
/// exactly one predecessor edge, duplicate edges counted.
fn single_predecessor<S: BlockTerminationState>(
    module: &Module<DynBrand, Unverified>,
    function: &str,
    block: &BasicBlock<'_, Dyn, S, DynBrand>,
) -> Option<BlockId<Dyn, DynBrand>> {
    let function = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let cfg = FunctionCfg::new(module.view(function));
    let mut predecessors = cfg.predecessors(block.id());
    let first = predecessors.next()?;
    predecessors.next().is_none().then_some(first)
}

/// The names of the block `block`'s predecessors in `function`, one per edge,
/// sorted.
fn predecessor_names(
    module: &Module<DynBrand, Unverified>,
    function: &str,
    block: &str,
) -> Vec<String> {
    let id = block_by_name(module, function, block).id();
    let function_id = module
        .function_dyn(function)
        .unwrap_or_else(|| panic!("fixture defines @{function}"));
    let cfg = FunctionCfg::new(module.view(function_id));
    let mut names: Vec<String> = cfg
        .predecessors(id)
        .map(|predecessor| {
            block_by_id(module, function, predecessor)
                .name()
                .unwrap_or_default()
        })
        .collect();
    names.sort();
    names
}

/// `DILocation::getAtomGroup` of `instruction`'s location: `None` when it has
/// no location (where upstream's `ASSERT_TRUE(DL)` fails), otherwise the
/// `atomGroup:` field, which upstream defaults to 0.
fn atom_group(
    module: &Module<DynBrand, Unverified>,
    instruction: &InstructionView<'_, DynBrand>,
) -> Option<i128> {
    let location = instruction.metadata().get(&MetadataAttachmentKind::Dbg)?;
    let Some(MetadataKind::Specialized(node)) = module.metadata_get(location) else {
        panic!("a `!dbg` attachment is a DILocation node");
    };
    Some(
        node.fields()
            .iter()
            .find(|field| field.name() == "atomGroup")
            .map_or(0, |field| match field.value() {
                MetadataFieldValue::Integer(group) => *group,
                other => panic!("atomGroup is an integer field, got {other:?}"),
            }),
    )
}

/// The textual body of `define … @function`, from `define` to its closing
/// brace, so a comparison is not coupled to the module header.
fn printed_function(module: &Module<DynBrand, Unverified>, function: &str) -> String {
    let text = format!("{module}");
    let start = text
        .find(&format!("@{function}("))
        .and_then(|at| text[..at].rfind("define"))
        .unwrap_or_else(|| panic!("@{function} is printed:\n{text}"));
    let end = text[start..]
        .find("\n}\n")
        .map(|at| start + at + "\n}\n".len())
        .unwrap_or_else(|| panic!("@{function} has a closing brace:\n{text}"));
    text[start..end].to_owned()
}

/// A labelled block header as `AsmWriter` prints it: the label, padded to
/// column 50, then the `; preds = …` comment.
fn label(name: &str, predecessors: &str) -> String {
    format!("{:<50}; preds = {predecessors}", format!("{name}:"))
}

/// `BasicBlock::splitBasicBlock` creates the new block right after the original
/// (`this->getNextNode()`), moves the split point and everything after it into
/// the new block, appends `br label %New` to the original, and rewrites the phis
/// of the moved terminator's successors to name the new block
/// (`New->replaceSuccessorsPhiUsesWith(this, New)`).
///
/// llvmkit-specific: no upstream unit test drives the non-debug effect of
/// `splitBasicBlock`, so the expected text is that routine's effect written
/// out. `split_at` used to append the new block at the end of the function,
/// insert no branch and leave the phi naming `%entry`.
#[test]
fn split_at_places_the_new_block_after_the_original_and_branches_to_it() -> Result<(), IrError> {
    let m = parse(
        r#"
define i32 @f(i32 %a) {
entry:
  %x = add i32 %a, 1
  %y = mul i32 %x, 2
  br label %exit

exit:
  %r = phi i32 [ %y, %entry ]
  ret i32 %r
}
"#,
    );
    let entry = block_by_name(&m, "f", "entry");
    let y = entry
        .instructions()
        .nth(1)
        .expect("entry holds %x, %y and the br");

    let tail = entry.split_at(&m, &y, "tail")?;

    assert_eq!(tail.name().as_deref(), Some("tail"));
    let expected = [
        "define i32 @f(i32 %a) {".to_owned(),
        "entry:".to_owned(),
        "  %x = add i32 %a, 1".to_owned(),
        "  br label %tail".to_owned(),
        String::new(),
        label("tail", "%entry"),
        "  %y = mul i32 %x, 2".to_owned(),
        "  br label %exit".to_owned(),
        String::new(),
        label("exit", "%tail"),
        "  %r = phi i32 [ %y, %tail ]".to_owned(),
        "  ret i32 %r".to_owned(),
        "}".to_owned(),
        String::new(),
    ]
    .join("\n");
    assert_eq!(printed_function(&m, "f"), expected);
    m.verify()
        .expect("the split leaves the function well formed");
    Ok(())
}

/// `BasicBlock::replacePhiUsesWith` calls `PHINode::replaceIncomingBlockWith`,
/// which rewrites *every* incoming entry naming the old block, and
/// `replaceSuccessorsPhiUsesWith` visits each successor edge, duplicates
/// included. A `switch` reaching `%exit` twice, with a phi holding one entry per
/// edge, must come out with both entries naming the new block.
///
/// llvmkit-specific: the duplicate-edge arm of `splitBasicBlock`'s phi rewrite
/// has no upstream unit test.
#[test]
fn split_at_rewrites_every_successor_phi_entry_naming_the_original() -> Result<(), IrError> {
    let m = parse(
        r#"
define i32 @g(i32 %a) {
entry:
  %y = add i32 %a, 1
  switch i32 %a, label %exit [
    i32 0, label %exit
  ]

exit:
  %r = phi i32 [ %y, %entry ], [ %y, %entry ]
  ret i32 %r
}
"#,
    );
    let entry = block_by_name(&m, "g", "entry");
    let switch = entry.terminator().expect("entry ends in the switch");

    let tail = entry.split_at(&m, &switch, "tail")?;
    let tail_id = tail.id();

    let exit = block_by_name(&m, "g", "exit");
    let phi = exit
        .instructions()
        .next()
        .expect("exit starts with the phi");
    let Some(InstructionKind::Phi(phi)) = phi.kind() else {
        panic!("exit starts with the phi");
    };
    assert_eq!(phi.incoming_count(), 2);
    for index in 0..phi.incoming_count() {
        assert_eq!(phi.incoming(index)?.1, tail_id, "incoming entry {index}");
    }
    Ok(())
}

/// `splitBasicBlock` splices `[I, end())` with `I` taken from
/// `Instruction::getIterator`, whose head bit is clear. `spliceDebugInfoImpl`
/// therefore leaves the debug records in front of `I` behind in the original
/// block (`!ReadFromHead && First->hasDbgRecords()`, onto `Src->end()`), and
/// the `br` appended next adopts them (`Instruction::insertBefore` and
/// `flushTerminatorDbgRecords`). They end up in front of the new branch, not in
/// front of the moved instruction.
///
/// llvmkit-specific: derived from those routines. The one upstream test that
/// pins record placement across a split, `BasicBlockDbgInfoTest`'s
/// `SplitBasicBlockBefore`, writes its records as `llvm.dbg.declare` calls,
/// which only the unported AutoUpgrade turns into records.
#[test]
fn split_at_leaves_the_split_points_debug_records_ahead_of_the_new_branch() -> Result<(), IrError> {
    let m = parse(
        r#"
define dso_local void @func(i32 %a) !dbg !10 {
entry:
  %x = add i32 %a, 1
    #dbg_value(i32 %x, !14, !DIExpression(), !16)
  %y = mul i32 %x, 2
  ret void, !dbg !17
}

!llvm.dbg.cu = !{!0}
!llvm.module.flags = !{!2, !3}

!0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, producer: "dummy", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, splitDebugInlining: false, nameTableKind: None)
!1 = !DIFile(filename: "dummy", directory: "dummy")
!2 = !{i32 7, !"Dwarf Version", i32 5}
!3 = !{i32 2, !"Debug Info Version", i32 3}
!10 = distinct !DISubprogram(name: "func", scope: !1, file: !1, line: 1, type: !11, scopeLine: 1, spFlags: DISPFlagDefinition, unit: !0, retainedNodes: !13)
!11 = !DISubroutineType(types: !12)
!12 = !{null}
!13 = !{}
!14 = !DILocalVariable(name: "a", scope: !10, file: !1, line: 2, type: !15)
!15 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
!16 = !DILocation(line: 2, column: 6, scope: !10)
!17 = !DILocation(line: 3, column: 2, scope: !10)
"#,
    );
    let entry = block_by_name(&m, "func", "entry");
    let y = entry
        .instructions()
        .nth(1)
        .expect("entry holds %x, %y and the ret");
    assert_eq!(
        y.debug_records().count(),
        1,
        "fixture: the record precedes %y"
    );

    let tail = entry.split_at(&m, &y, "tail")?;

    let moved = tail.instructions().next().expect("tail starts with %y");
    assert_eq!(
        moved.debug_records().count(),
        0,
        "the record did not move with %y"
    );
    let entry = block_by_name(&m, "func", "entry");
    let branch = entry.terminator().expect("entry ends in the new br");
    assert_eq!(
        branch.debug_records().count(),
        1,
        "the new br adopted the record"
    );
    Ok(())
}

/// Port of `unittests/IR/BasicBlockDbgInfoTest.cpp`'s
/// `TEST(BasicBlockDbgInfoTest, DropSourceAtomOnSplit)`, IR verbatim, both
/// halves in upstream's order. Each split gives the branch it inserts the
/// split point's location without its atom (`DILocation::getWithoutAtom`),
/// while the moved `ret` keeps atom group 1.
#[test]
fn drop_source_atom_on_split() -> Result<(), IrError> {
    let m = parse(
        r#"
    define dso_local void @func() !dbg !10 {
      %1 = alloca i32, align 4
      ret void, !dbg !DILocation(line: 3, column: 2, scope: !10, atomGroup: 1, atomRank: 1)
    }

    !llvm.dbg.cu = !{!0}
    !llvm.module.flags = !{!2, !3}

    !0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, producer: "dummy", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, splitDebugInlining: false, nameTableKind: None)
    !1 = !DIFile(filename: "dummy", directory: "dummy")
    !2 = !{i32 7, !"Dwarf Version", i32 5}
    !3 = !{i32 2, !"Debug Info Version", i32 3}
    !10 = distinct !DISubprogram(name: "func", scope: !1, file: !1, line: 1, type: !11, scopeLine: 1, spFlags: DISPFlagDefinition, unit: !0, retainedNodes: !13, keyInstructions: true)
    !11 = !DISubroutineType(types: !12)
    !12 = !{null}
    !13 = !{}
    !14 = !DILocalVariable(name: "a", scope: !10, file: !1, line: 2, type: !15)
    !15 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
  "#,
    );
    let func = m.function_dyn("func").expect("fixture defines @func");

    // Test splitBasicBlockBefore.
    {
        let bb = m.view(func).basic_blocks().next_back().expect("F->back()");
        // Split at `ret void`.
        let split_point = bb.terminator().expect("std::prev(BB.end(), 1)");
        let before = bb.split_before(&m, &split_point, "before")?;
        let br_to_after = before.terminator().expect("Before->getTerminator()");
        assert_eq!(atom_group(&m, &br_to_after), Some(0));

        let after = single_successor(&before).expect("Before->getSingleSuccessor()");
        let after = block_by_id(&m, "func", after);
        let orig_terminator = after.terminator().expect("After->getTerminator()");
        assert_eq!(atom_group(&m, &orig_terminator), Some(1));
    }

    // Test splitBasicBlock.
    {
        let bb = m.view(func).basic_blocks().next_back().expect("F->back()");
        // Split at `ret void`.
        let split_point = bb.terminator().expect("std::prev(BB.end(), 1)");
        let after = bb.split_at(&m, &split_point, "before")?;

        let orig_terminator = after.terminator().expect("After->getTerminator()");
        assert_eq!(atom_group(&m, &orig_terminator), Some(1));

        let before = single_predecessor(&m, "func", &after).expect("After->getSinglePredecessor()");
        let before = block_by_id(&m, "func", before);
        let br_to_after = before.terminator().expect("Before->getTerminator()");
        assert_eq!(atom_group(&m, &br_to_after), Some(0));
    }
    Ok(())
}

/// Port of `unittests/Transforms/Utils/BasicBlockUtilsTest.cpp`'s
/// `TEST(BasicBlockUtils, splitBasicBlockBefore_ex1)`, IR verbatim. Splitting
/// `bb2` before its phi moves nothing, points `bb0`'s branch at the new block,
/// and rewrites the phi's incoming block to it.
#[test]
fn split_basic_block_before_ex1() -> Result<(), IrError> {
    let m = parse(
        r#"
define void @foo() {
bb0:
 %0 = mul i32 1, 2
  br label %bb2
bb1:
  br label %bb3
bb2:
  %1 = phi  i32 [ %0, %bb0 ]
  br label %bb3
bb3:
  ret void
}
"#,
    );
    let foo = m.function_dyn("foo").expect("fixture defines @foo");
    let _dominator_tree = DominatorTree::new(m.view(foo));

    let dest_block = block_by_name(&m, "foo", "bb2");
    let front = dest_block
        .instructions()
        .next()
        .expect("DestBlock->front()");
    let new_bb = dest_block.split_before(&m, &front, "test")?;

    let dest_block = block_by_name(&m, "foo", "bb2");
    let front = dest_block
        .instructions()
        .next()
        .expect("DestBlock->front()");
    let Some(InstructionKind::Phi(pn)) = front.kind() else {
        panic!("dyn_cast<PHINode>(&(DestBlock->front()))");
    };
    assert_eq!(pn.incoming(0)?.1, new_bb.id());
    assert_eq!(new_bb.name().as_deref(), Some("test"));
    assert_eq!(single_successor(&new_bb), Some(dest_block.id()));
    assert_eq!(
        single_predecessor(&m, "foo", &dest_block),
        Some(new_bb.id())
    );
    Ok(())
}

/// Port of `unittests/Transforms/Utils/BasicBlockUtilsTest.cpp`'s
/// `TEST(BasicBlockUtils, splitBasicBlockBefore_ex2)`, IR verbatim. Upstream
/// is an `#ifndef NDEBUG` `ASSERT_DEATH` on the assert "cannot split on multi
/// incoming phis": `bb2`'s phi has two predecessors. llvmkit does not port
/// the crash: `split_before` returns that text as `IrError::InvalidOperation`
/// and leaves the module untouched, which is hardening rather than divergence.
#[test]
fn split_basic_block_before_ex2() {
    let m = parse(
        r#"
define void @foo() {
bb0:
 %0 = mul i32 1, 2
  br label %bb2
bb1:
  br label %bb2
bb2:
  %1 = phi  i32 [ %0, %bb0 ], [ 1, %bb1 ]
  br label %bb3
bb3:
  ret void
}
"#,
    );
    let foo = m.function_dyn("foo").expect("fixture defines @foo");
    let _dominator_tree = DominatorTree::new(m.view(foo));

    let dest_block = block_by_name(&m, "foo", "bb2");
    let front = dest_block
        .instructions()
        .next()
        .expect("DestBlock->front()");
    let printed_before = format!("{m}");

    let result = dest_block.split_before(&m, &front, "test");

    assert!(
        matches!(
            result,
            Err(IrError::InvalidOperation {
                message: "cannot split on multi incoming phis"
            })
        ),
        "split_before must refuse a phi with several predecessors"
    );
    assert_eq!(
        format!("{m}"),
        printed_before,
        "a refused split must not mutate the module"
    );
}

/// Stands in for `unittests/IR/BasicBlockDbgInfoTest.cpp`'s
/// `TEST(BasicBlockDbgInfoTest, SplitBasicBlockBefore)`, which cannot be ported
/// yet: it writes its record as `call void @llvm.dbg.declare(...)`, and only
/// AutoUpgrade turns that call into a record (`UpgradeCallsToIntrinsic` reaches
/// `upgradeDbgIntrinsicToDbgRecord` through `UpgradeIntrinsicCall`), which
/// llvmkit has not ported (`docs/future-work.md`, "1/2/4 — the
/// intrinsic-upgrade framework"). This is the same IR
/// with the record written as `#dbg_declare`, the same split and the same
/// assertion that the new branch carries the record. It additionally checks
/// that the record left the `store`, so it moved rather than being copied.
///
/// The upstream effect: `splitBasicBlockBefore` splices `[begin(), I)`, and
/// with `Last = I` and its tail bit clear `spliceDebugInfoImpl` moves `I`'s
/// records to the new block's end, where the branch inserted next adopts them.
#[test]
fn split_before_moves_the_split_points_debug_records_ahead_of_the_new_branch() -> Result<(), IrError>
{
    let m = parse(
        r#"
    define dso_local void @func() #0 !dbg !10 {
      %1 = alloca i32, align 4
        #dbg_declare(ptr %1, !14, !DIExpression(), !16)
      store i32 2, ptr %1, align 4, !dbg !16
      ret void, !dbg !17
    }

    declare void @llvm.dbg.declare(metadata, metadata, metadata) #0

    attributes #0 = { nocallback nofree nosync nounwind speculatable willreturn memory(none) }

    !llvm.dbg.cu = !{!0}
    !llvm.module.flags = !{!2, !3, !4, !5, !6, !7, !8}
    !llvm.ident = !{!9}

    !0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, producer: "dummy", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, splitDebugInlining: false, nameTableKind: None)
    !1 = !DIFile(filename: "dummy", directory: "dummy")
    !2 = !{i32 7, !"Dwarf Version", i32 5}
    !3 = !{i32 2, !"Debug Info Version", i32 3}
    !4 = !{i32 1, !"wchar_size", i32 4}
    !5 = !{i32 8, !"PIC Level", i32 2}
    !6 = !{i32 7, !"PIE Level", i32 2}
    !7 = !{i32 7, !"uwtable", i32 2}
    !8 = !{i32 7, !"frame-pointer", i32 2}
    !9 = !{!"dummy"}
    !10 = distinct !DISubprogram(name: "func", scope: !1, file: !1, line: 1, type: !11, scopeLine: 1, spFlags: DISPFlagDefinition, unit: !0, retainedNodes: !13)
    !11 = !DISubroutineType(types: !12)
    !12 = !{null}
    !13 = !{}
    !14 = !DILocalVariable(name: "a", scope: !10, file: !1, line: 2, type: !15)
    !15 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
    !16 = !DILocation(line: 2, column: 6, scope: !10)
    !17 = !DILocation(line: 3, column: 2, scope: !10)
  "#,
    );
    let func = m.function_dyn("func").expect("fixture defines @func");

    let bb = m.view(func).entry_block().expect("F->getEntryBlock()");
    let split_point = bb
        .instructions()
        .nth_back(1)
        .expect("std::prev(BB.end(), 2): store i32 2, ptr %1");
    bb.split_before(&m, &split_point, "before")?;

    let bb_before = m.view(func).entry_block().expect("F->getEntryBlock()");
    let branch = bb_before
        .terminator()
        .expect("std::prev(BBBefore.end()): br label %1 (new)");
    assert!(branch.debug_records().count() > 0, "I2->hasDbgRecords()");

    let bb = block_by_id(
        &m,
        "func",
        single_successor(&bb_before).expect("the new block branches to the old one"),
    );
    let store = bb
        .instructions()
        .next()
        .expect("the old block starts with the store");
    assert_eq!(
        store.debug_records().count(),
        0,
        "the record moved off the store"
    );
    Ok(())
}

/// Splits the block `t` of each of `functions` in `module` before its first
/// instruction, then checks that every edge that entered `t` enters the new
/// block instead — read both off the use list (the predecessors) and off the
/// terminators' successor slots — and that the new block's branch is `t`'s
/// only predecessor. Every function is checked and every miss reported
/// together, so one run names each terminator kind that failed.
fn assert_split_before_retargets_every_edge(
    module: &Module<DynBrand, Unverified>,
    functions: &[&str],
) -> Result<(), IrError> {
    let mut misses = Vec::new();
    for &function in functions {
        let edges_into_t = predecessor_names(module, function, "t");
        assert!(!edges_into_t.is_empty(), "@{function}: fixture reaches %t");
        let t = block_by_name(module, function, "t");
        let first = t.instructions().next().expect("%t is not empty");

        t.split_before(module, &first, "new")?;

        let edges_into_new = predecessor_names(module, function, "new");
        if edges_into_new != edges_into_t {
            misses.push(format!(
                "@{function}: edges into %new are {edges_into_new:?}, not {edges_into_t:?}"
            ));
        }
        let edges_into_t_now = predecessor_names(module, function, "t");
        if edges_into_t_now != ["new"] {
            misses.push(format!(
                "@{function}: edges into %t are {edges_into_t_now:?}, not only %new's branch"
            ));
        }
        let t_id = block_by_name(module, function, "t").id();
        let function_id = module.function_dyn(function).expect("fixture function");
        for block in module.view(function_id).basic_blocks() {
            if block.name().as_deref() != Some("new")
                && block.successors().any(|successor| successor == t_id)
            {
                misses.push(format!(
                    "@{function}: %{} still names %t as a successor",
                    block.name().unwrap_or_default()
                ));
            }
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
    Ok(())
}

/// `splitBasicBlockBefore` calls `TI->replaceSuccessorWith(this, New)` for each
/// entry of `predecessors(this)`, and `Instruction::replaceSuccessorWith`
/// rewrites every successor slot naming the old block. One function per
/// ordinary terminator kind; `br`, `switch`, `indirectbr` and `callbr` name the
/// split block from two slots.
///
/// llvmkit-specific: no upstream unit test drives `replaceSuccessorWith` per
/// terminator kind through this routine.
#[test]
fn split_before_retargets_every_successor_slot_of_ordinary_terminators() -> Result<(), IrError> {
    let m = parse(
        r#"
declare void @callee()
declare i32 @__gxx_personality_v0(...)

define void @br(i1 %c) {
entry:
  br i1 %c, label %t, label %t
t:
  ret void
}

define void @switch(i32 %v) {
entry:
  switch i32 %v, label %t [
    i32 1, label %t
    i32 2, label %other
  ]
t:
  ret void
other:
  ret void
}

define void @indirectbr(ptr %address) {
entry:
  indirectbr ptr %address, [label %t, label %t]
t:
  ret void
}

define void @invoke() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @callee() to label %t unwind label %lpad
t:
  ret void
lpad:
  %l = landingpad { ptr, i32 } cleanup
  ret void
}

define void @callbr() {
entry:
  callbr void @callee() to label %t [label %t]
t:
  ret void
}
"#,
    );
    assert_split_before_retargets_every_edge(
        &m,
        &["br", "switch", "indirectbr", "invoke", "callbr"],
    )
}

/// The exception-handling successor slots of
/// [`split_before_retargets_every_successor_slot_of_ordinary_terminators`]:
/// `catchret`'s target, `cleanupret`'s unwind destination, a `catchswitch`
/// naming the split block both as a handler and as its unwind destination, and
/// an `invoke`'s unwind destination.
///
/// llvmkit-specific, for the same reason. The fixtures are well-formed only as
/// far as the parser checks; the verifier would reject the result of splitting
/// before an EH pad, as it would upstream, and is not run.
#[test]
fn split_before_retargets_every_successor_slot_of_exception_handling_terminators()
-> Result<(), IrError> {
    let m = parse(
        r#"
declare void @callee()
declare i32 @__gxx_personality_v0(...)

define void @catchret() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @callee() to label %exit unwind label %dispatch
dispatch:
  %cs = catchswitch within none [label %handler] unwind to caller
handler:
  %cp = catchpad within %cs [ptr null]
  catchret from %cp to label %t
t:
  ret void
exit:
  ret void
}

define void @cleanupret() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @callee() to label %exit unwind label %cleanup
cleanup:
  %cl = cleanuppad within none []
  cleanupret from %cl unwind label %t
t:
  %cs = catchswitch within none [label %handler] unwind to caller
handler:
  %cp = catchpad within %cs [ptr null]
  catchret from %cp to label %exit
exit:
  ret void
}

define void @catchswitch() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @callee() to label %exit unwind label %dispatch
dispatch:
  %cs = catchswitch within none [label %t] unwind label %t
t:
  %cp = catchpad within %cs [ptr null]
  catchret from %cp to label %exit
exit:
  ret void
}

define void @invoke_unwind() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @callee() to label %exit unwind label %t
t:
  %l = landingpad { ptr, i32 } cleanup
  ret void
exit:
  ret void
}
"#,
    );
    assert_split_before_retargets_every_edge(
        &m,
        &["catchret", "cleanupret", "catchswitch", "invoke_unwind"],
    )
}
