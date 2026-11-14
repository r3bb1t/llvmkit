//! A module's `LayoutToken` is the permission to change that module's layout,
//! and every entry that takes one refuses another module's.
//!
//! Two `Module::dynamic` modules share `DynBrand`, so their tokens have one
//! type and only the module tag tells them apart (D7). An edit that accepted
//! another module's token would change a module while that module's own
//! token, and so possibly a live builder, stood unaware of it.
//!
//! No upstream counterpart: LLVM's `IRBuilder` and `Instruction` mutators need
//! no permission. Design: `docs/design/layout-token-design.md`.

use llvmkit_ir::instruction::state::Attached;
use llvmkit_ir::{
    BasicBlock, DynBrand, Instruction, IrBuilder, IrError, LayoutToken, Linkage, Module, Terminated,
};

/// A block and its terminator, as an at-end `ret void` returns them.
struct Returning<'ctx> {
    block: BasicBlock<'ctx, (), Terminated, DynBrand>,
    ret: Instruction<'ctx, Attached, DynBrand>,
}

/// `void @name() { entry: ret void }` in `key`'s module.
fn returning<'ctx>(key: &mut LayoutToken<'ctx, DynBrand>, name: &str) -> Returning<'ctx> {
    let m = key.module();
    let f = m
        .add_typed_function::<(), (), _>(name, Linkage::External)
        .expect("declare")
        .as_function();
    let entry = m.view(f).append_basic_block(m, "entry");
    let (block, ret) = IrBuilder::new_for::<()>(key)
        .position_at_end(entry)
        .ret_void();
    Returning { block, ret }
}

/// Every keyed edit refuses a token of another module with
/// `IrError::ForeignLayoutToken`, before it reads or changes anything: the
/// edited module prints the same afterwards.
#[test]
fn every_keyed_edit_refuses_another_modules_token() {
    let mut owner_module = Module::dynamic("token-owner");
    let mut owner_key = owner_module.layout_token();
    let mut edited_module = Module::dynamic("edited");
    let mut edited_key = edited_module.layout_token();
    let edited = edited_key.module();

    let erased = returning(&mut edited_key, "erased").ret;
    let detached = returning(&mut edited_key, "detached").ret;
    let moved_before = returning(&mut edited_key, "moved_before").ret;
    let moved_after = returning(&mut edited_key, "moved_after").ret;
    let anchor = returning(&mut edited_key, "anchor").ret.placed();
    let inserted_before = returning(&mut edited_key, "inserted_before")
        .ret
        .detach_from_parent(&mut edited_key)
        .expect("the module's own token");
    let inserted_after = returning(&mut edited_key, "inserted_after")
        .ret
        .detach_from_parent(&mut edited_key)
        .expect("the module's own token");
    let appended = returning(&mut edited_key, "appended")
        .ret
        .detach_from_parent(&mut edited_key)
        .expect("the module's own token");
    let open_function = edited
        .add_typed_function::<(), (), _>("open", Linkage::External)
        .expect("declare")
        .as_function();
    let open = edited
        .view(open_function)
        .append_basic_block(edited, "open");
    let split_at = returning(&mut edited_key, "split_at");
    let split_before = returning(&mut edited_key, "split_before");
    let spliced = returning(&mut edited_key, "spliced");
    let splice_dest = returning(&mut edited_key, "splice_dest").block;
    let before = format!("{edited}");

    let outcomes: Vec<(&str, Result<(), IrError>)> = vec![
        (
            "erase_from_parent",
            erased.erase_from_parent(&mut owner_key),
        ),
        (
            "detach_from_parent",
            detached.detach_from_parent(&mut owner_key).map(|_| ()),
        ),
        (
            "move_before",
            moved_before.move_before(&mut owner_key, anchor),
        ),
        ("move_after", moved_after.move_after(&mut owner_key, anchor)),
        (
            "insert_before",
            inserted_before
                .insert_before(&mut owner_key, anchor)
                .map(|_| ()),
        ),
        (
            "insert_after",
            inserted_after
                .insert_after(&mut owner_key, anchor)
                .map(|_| ()),
        ),
        (
            "append_to",
            appended.append_to(&mut owner_key, &open).map(|_| ()),
        ),
        (
            "split_at",
            split_at
                .block
                .split_at(&mut owner_key, &split_at.ret.as_view(), "tail")
                .map(|_| ()),
        ),
        (
            "split_before",
            split_before
                .block
                .split_before(&mut owner_key, &split_before.ret.as_view(), "head")
                .map(|_| ()),
        ),
        (
            "splice_into",
            spliced.block.splice_into(&mut owner_key, splice_dest),
        ),
    ];
    let let_through: Vec<String> = outcomes
        .into_iter()
        .filter(|(_, outcome)| *outcome != Err(IrError::ForeignLayoutToken))
        .map(|(entry, outcome)| format!("{entry}: {outcome:?}"))
        .collect();
    assert!(
        let_through.is_empty(),
        "each entry must refuse another module's token: {let_through:#?}"
    );
    assert_eq!(
        format!("{edited}"),
        before,
        "a refused edit changes nothing"
    );
}

/// `IrBuilder::at_end` refuses a block of another module than its token's:
/// the builder's arena is the token's module, so it must not write into
/// another module's block. `BasicBlock::builder` is the same entry.
#[test]
fn at_end_refuses_a_block_of_another_module() {
    let mut owner_module = Module::dynamic("token-owner");
    let mut owner_key = owner_module.layout_token();
    let mut edited_module = Module::dynamic("edited");
    let edited_key = edited_module.layout_token();
    let edited = edited_key.module();
    let f = edited
        .add_typed_function::<(), (), _>("f", Linkage::External)
        .expect("declare")
        .as_function();
    let first = edited.view(f).append_basic_block(edited, "first");
    let second = edited.view(f).append_basic_block(edited, "second");

    assert_eq!(
        IrBuilder::at_end(&mut owner_key, first).err(),
        Some(IrError::ForeignLayoutToken)
    );
    assert_eq!(
        second.builder(&mut owner_key).err(),
        Some(IrError::ForeignLayoutToken)
    );
}
