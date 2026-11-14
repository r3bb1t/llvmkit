//! A module's `LayoutToken` is the permission to change that module's layout,
//! and every entry that takes one alongside a handle refuses a handle of
//! another module than the token's.
//!
//! Two `Module::dynamic` modules share `DynBrand`, so their tokens have one
//! type and only the module tag tells them apart (D7). An edit that accepted
//! another module's token would change a module while that module's own
//! token, and so possibly a live builder, stood unaware of it.
//!
//! The entries, derived at `95febe47` (plus `try_position_at_end`, which takes
//! no token but positions a token-holding builder) with
//! `grep -rn -B6 "key: &'[a-z]* mut \(crate::\)\?LayoutToken\|key: &mut \(crate::\)\?LayoutToken" crates/llvmkit-ir/src --include=*.rs | grep -oE "pub (\(crate\) )?fn [a-z_]+" | sort | uniq -c`:
//! the ten layout edits (`erase_from_parent`, `detach_from_parent`,
//! `move_before`, `move_after`, `insert_before`, `insert_after`, `append_to`,
//! `split_at`, `split_before`, `splice_into`) are the first test, and the
//! eight of them that take a second handle (all but `erase_from_parent` and
//! `detach_from_parent`) the second; the entries that bind a builder to a
//! handle (`IrBuilder::at_end`, `BasicBlock::builder`,
//! `SsaBuilder::for_function` / `with_folder_for_function`) are the third.
//! The rest take a token and no
//! handle (`IrBuilder::new` / `new_for` / `new_for_return` / `with_folder`,
//! and the two `TypedFunctionValue::builder`s, which read only the function's
//! type): they return an unpositioned builder, and positioning it is where a
//! block is compared.
//!
//! No upstream counterpart: LLVM's `IRBuilder` and `Instruction` mutators need
//! no permission. Design: `docs/design/layout-token-design.md`.

use llvmkit_ir::instruction::state::Attached;
use llvmkit_ir::{
    BasicBlock, ConstantFolder, DynBrand, Instruction, IrBuilder, IrError, LayoutToken, Linkage,
    Module, SsaBuilder, SsaState, Terminated,
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
        .try_position_at_end(entry)
        .expect("the block is the builder's module's")
        .ret_void();
    Returning { block, ret }
}

/// Each of the ten layout edits (the list and its derivation are in the file
/// header) refuses a token of another module with
/// `IrError::ForeignLayoutToken`, the error that names the token argument,
/// before it reads or changes anything: the edited module prints the same
/// afterwards.
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

/// Each of the eight layout edits that take a second handle — the ten in the
/// file header but `erase_from_parent` and `detach_from_parent`; an anchor, a
/// destination block or a split point — refuses a foreign token with
/// `IrError::ForeignLayoutToken` even when that second handle is foreign
/// too: the token is checked first, so the error names the token argument.
/// The second handles here are all of the token's module, which is not the
/// edited one; checked second, they would answer `ForeignValueId`. Neither
/// module changes.
#[test]
fn a_foreign_token_is_named_before_a_foreign_second_handle() {
    let mut owner_module = Module::dynamic("token-owner");
    let mut owner_key = owner_module.layout_token();
    let owner = owner_key.module();
    let mut edited_module = Module::dynamic("edited");
    let mut edited_key = edited_module.layout_token();
    let edited = edited_key.module();

    // The second handles, all of the token's module.
    let foreign_anchor = returning(&mut owner_key, "foreign_anchor").ret.placed();
    let foreign_dest = returning(&mut owner_key, "foreign_dest").block;
    let foreign_open_function = owner
        .add_typed_function::<(), (), _>("foreign_open", Linkage::External)
        .expect("declare")
        .as_function();
    let foreign_open = owner
        .view(foreign_open_function)
        .append_basic_block(owner, "foreign_open");
    let foreign_split_point = returning(&mut owner_key, "foreign_split_point").ret;

    // The edited handles, of the other module.
    let moved_before = returning(&mut edited_key, "moved_before").ret;
    let moved_after = returning(&mut edited_key, "moved_after").ret;
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
    let split_at = returning(&mut edited_key, "split_at").block;
    let split_before = returning(&mut edited_key, "split_before").block;
    let spliced = returning(&mut edited_key, "spliced").block;
    let before = format!("{edited}");
    let owner_before = format!("{owner}");

    let outcomes: Vec<(&str, Result<(), IrError>)> = vec![
        (
            "move_before",
            moved_before.move_before(&mut owner_key, foreign_anchor),
        ),
        (
            "move_after",
            moved_after.move_after(&mut owner_key, foreign_anchor),
        ),
        (
            "insert_before",
            inserted_before
                .insert_before(&mut owner_key, foreign_anchor)
                .map(|_| ()),
        ),
        (
            "insert_after",
            inserted_after
                .insert_after(&mut owner_key, foreign_anchor)
                .map(|_| ()),
        ),
        (
            "append_to",
            appended
                .append_to(&mut owner_key, &foreign_open)
                .map(|_| ()),
        ),
        (
            "split_at",
            split_at
                .split_at(&mut owner_key, &foreign_split_point.as_view(), "tail")
                .map(|_| ()),
        ),
        (
            "split_before",
            split_before
                .split_before(&mut owner_key, &foreign_split_point.as_view(), "head")
                .map(|_| ()),
        ),
        (
            "splice_into",
            spliced.splice_into(&mut owner_key, foreign_dest),
        ),
    ];
    let let_through: Vec<String> = outcomes
        .into_iter()
        .filter(|(_, outcome)| *outcome != Err(IrError::ForeignLayoutToken))
        .map(|(entry, outcome)| format!("{entry}: {outcome:?}"))
        .collect();
    assert!(
        let_through.is_empty(),
        "each entry must name the foreign token first: {let_through:#?}"
    );
    assert_eq!(
        format!("{edited}"),
        before,
        "a refused edit changes nothing"
    );
    assert_eq!(
        format!("{owner}"),
        owner_before,
        "a refused edit changes nothing on its second handle's side either"
    );
}

/// Each entry that binds a token-holding builder to a handle refuses a handle
/// of another module than the token's: the builder's arena is the token's
/// module, so it must not write into another module's block or function.
/// Each refuses with `IrError::ForeignValueId`, the handle-of-another-module
/// error every positioning entry and every function boundary share; a token
/// argument of another module is `IrError::ForeignLayoutToken`, the layout
/// edits' (the first test).
#[test]
fn builder_bindings_refuse_a_handle_of_another_module() {
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
    let third = edited.view(f).append_basic_block(edited, "third");
    let declared = edited
        .add_typed_function::<(), (), _>("declared", Linkage::External)
        .expect("declare")
        .as_function();
    let mut state = SsaState::for_function(edited, edited.view(declared)).expect("a declaration");

    let outcomes: Vec<(&str, Option<IrError>)> = vec![
        (
            "IrBuilder::at_end",
            IrBuilder::at_end(&mut owner_key, first).err(),
        ),
        ("BasicBlock::builder", second.builder(&mut owner_key).err()),
        (
            "try_position_at_end",
            IrBuilder::new_for::<()>(&mut owner_key)
                .try_position_at_end(third)
                .err(),
        ),
        (
            "SsaBuilder::for_function",
            SsaBuilder::for_function(&mut owner_key, edited.view(declared), &mut state).err(),
        ),
        (
            "SsaBuilder::with_folder_for_function",
            SsaBuilder::with_folder_for_function(
                &mut owner_key,
                edited.view(declared),
                &mut state,
                ConstantFolder,
            )
            .err(),
        ),
    ];
    let let_through: Vec<String> = outcomes
        .into_iter()
        .filter(|(_, outcome)| *outcome != Some(IrError::ForeignValueId))
        .map(|(entry, outcome)| format!("{entry}: {outcome:?}"))
        .collect();
    assert!(let_through.is_empty(), "{let_through:#?}");
}
