//! Use-list order directives — `uselistorder` and `uselistorder_bb`.
//!
//! Mirrors `LLParser.cpp`'s `parseUseListOrder`, `parseUseListOrderBB`,
//! `parseUseListOrderIndexes` and `sortUseListOrder`, and the printing half
//! in `AsmWriter.cpp` (`predictUseListOrder` / `printUseListOrder`).
//!
//! The negatives assert `err.to_string()` against the fixture's own `CHECK`
//! line, because upstream's wording is contractual. The positives go through
//! [`llvmkit_ir::Module::preserving_use_list_order`], which is
//! `Module::print` with `ShouldPreserveUseListOrder` set — plain `Display`
//! mirrors the default and emits no directives at all, exactly as `llvm-dis`
//! prints.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

/// Parse `src` expecting rejection, and return the rendered diagnostic.
fn parse_err(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    match Parser::new(src, &module) {
        Err(e) => e.to_string(),
        Ok(parser) => parser
            .parse_module()
            .expect_err("fixture is rejected")
            .to_string(),
    }
}

/// Parse `src`, then print it with use-list order preserved.
fn parse_and_print_preserving(module_name: &str, src: &[u8]) -> String {
    let module = Module::dynamic(module_name);
    Parser::new(src, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");
    module.preserving_use_list_order().to_string()
}

// --------------------------------------------------------------------------
// `parseUseListOrderIndexes`
// --------------------------------------------------------------------------

/// Ports `test/Assembler/invalid-uselistorder-indexes-ordered.ll`, whose
/// CHECK line is `error: expected uselistorder indexes to change the order`
/// — `parseUseListOrderIndexes`'s `IsOrdered` accumulator.
#[test]
fn ordered_indexes_are_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_ordered.ll");

    assert_eq!(
        parse_err("indexes_ordered", FIXTURE),
        "expected uselistorder indexes to change the order"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-duplicated.ll`, whose
/// CHECK line is
/// `error: expected distinct uselistorder indexes in range [0, size)`.
///
/// `{ 0, 0, 2 }` is caught by upstream's `Offset` accumulator, not by `Max`:
/// the indexes sum to 2 where `0 + 1 + 2` is 3.
#[test]
fn duplicated_indexes_are_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_duplicated.ll");

    assert_eq!(
        parse_err("indexes_duplicated", FIXTURE),
        "expected distinct uselistorder indexes in range [0, size)"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-range.ll`, the same
/// message reached the other way: `{ 0, 3, 1 }` sums correctly but `Max`
/// is not below the vector's length.
#[test]
fn out_of_range_indexes_are_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_range.ll");

    assert_eq!(
        parse_err("indexes_range", FIXTURE),
        "expected distinct uselistorder indexes in range [0, size)"
    );
}

// --------------------------------------------------------------------------
// `sortUseListOrder`
// --------------------------------------------------------------------------

/// Ports `test/Assembler/invalid-uselistorder-global-missing.ll`, whose
/// CHECK line is `error: value has no uses`. The `@global` it names is a
/// forward reference nothing ever defines or reads.
#[test]
fn directive_on_an_unused_global_is_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_global_missing.ll");

    assert_eq!(parse_err("global_missing", FIXTURE), "value has no uses");
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-empty.ll`. Despite the
/// fixture's name its CHECK line is `error: value has no uses`: `@global` is
/// defined but never referenced.
#[test]
fn directive_on_a_defined_but_unused_global_is_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_empty.ll");

    assert_eq!(parse_err("indexes_empty", FIXTURE), "value has no uses");
}

/// Ports `test/Assembler/invalid-uselistorder-function-missing-named.ll`,
/// whose CHECK line is `error: value has no uses` — a function-local name
/// that is forward-referenced by the directive itself and never defined.
#[test]
fn directive_on_an_unused_local_name_is_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/uselistorder/invalid_uselistorder_function_missing_named.ll"
    );

    assert_eq!(
        parse_err("function_missing_named", FIXTURE),
        "value has no uses"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-function-missing-numbered.ll`,
/// the numbered twin of [`directive_on_an_unused_local_name_is_rejected`].
#[test]
fn directive_on_an_unused_numbered_local_is_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/uselistorder/invalid_uselistorder_function_missing_numbered.ll"
    );

    assert_eq!(
        parse_err("function_missing_numbered", FIXTURE),
        "value has no uses"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-one.ll`, whose CHECK
/// line is `error: value only has one use` — `sortUseListOrder`'s
/// `NumUses < 2` arm, distinct from the empty case above.
#[test]
fn directive_on_a_singly_used_value_is_rejected() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_one.ll");

    assert_eq!(parse_err("indexes_one", FIXTURE), "value only has one use");
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-toofew.ll`, whose CHECK
/// line is `error: wrong number of indexes, expected 3`. The count in the
/// message is the value's *actual* use count, not the vector's length.
#[test]
fn too_few_indexes_name_the_actual_use_count() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_toofew.ll");

    assert_eq!(
        parse_err("indexes_toofew", FIXTURE),
        "wrong number of indexes, expected 3"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-indexes-toomany.ll`, whose
/// CHECK line is `error: wrong number of indexes, expected 2` — the same
/// message reached from the other side.
#[test]
fn too_many_indexes_name_the_actual_use_count() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_indexes_toomany.ll");

    assert_eq!(
        parse_err("indexes_toomany", FIXTURE),
        "wrong number of indexes, expected 2"
    );
}

// --------------------------------------------------------------------------
// `parseUseListOrder` framing
// --------------------------------------------------------------------------

/// Ports `test/Assembler/invalid-uselistorder-type.ll`, whose CHECK line is
/// `error: '%x' defined with type 'i32' but expected 'float'`.
///
/// The directive reads its operand with the general `parseTypeAndValue`, so
/// a type disagreement is reported by `checkValidVariableType` before any
/// use-list rule runs.
#[test]
fn directive_operand_type_must_agree() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_type.ll");

    assert_eq!(
        parse_err("uselistorder_type", FIXTURE),
        "'%x' defined with type 'i32' but expected 'float'"
    );
}

/// Ports `test/Assembler/invalid-uselistorder-function-between-blocks.ll`,
/// whose CHECK line is `error: expected uselistorder directive`.
///
/// `parseFunctionBody` runs *two* sequential loops — every basic block, then
/// every directive — so a block header after a directive has nowhere to go.
/// Reaching that message also requires the five directives ahead of it to
/// parse, including `uselistorder i32 7, { 1, 0 }`: a `ConstantInt` has no
/// use list at all (`Value::hasUseList`), so `sortUseListOrder` accepts it
/// and does nothing.
#[test]
fn a_block_after_a_directive_is_rejected() {
    const FIXTURE: &[u8] = include_bytes!(
        "fixtures/upstream/uselistorder/invalid_uselistorder_function_between_blocks.ll"
    );

    assert_eq!(
        parse_err("function_between_blocks", FIXTURE),
        "expected uselistorder directive"
    );
}

// --------------------------------------------------------------------------
// `parseUseListOrderBB`
// --------------------------------------------------------------------------

/// Ports `test/Assembler/invalid-uselistorder_bb-missing-func.ll`, whose
/// CHECK line is
/// `error: invalid function forward reference in uselistorder_bb`.
#[test]
fn uselistorder_bb_rejects_an_undefined_function() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_missing_func.ll");

    assert_eq!(
        parse_err("bb_missing_func", FIXTURE),
        "invalid function forward reference in uselistorder_bb"
    );
}

/// Ports `test/Assembler/invalid-uselistorder_bb-not-func.ll`, whose CHECK
/// line is `error: expected function name in uselistorder_bb`.
///
/// `Module::getNamedValue` finds `@global`, so the name resolves and only
/// then fails the `dyn_cast<Function>` — a different verdict from the
/// unresolved case above.
#[test]
fn uselistorder_bb_rejects_a_non_function_global() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_not_func.ll");

    assert_eq!(
        parse_err("bb_not_func", FIXTURE),
        "expected function name in uselistorder_bb"
    );
}

/// Ports `test/Assembler/invalid-uselistorder_bb-missing-body.ll`, whose
/// CHECK line is `error: invalid declaration in uselistorder_bb`.
#[test]
fn uselistorder_bb_rejects_a_declaration() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_missing_body.ll");

    assert_eq!(
        parse_err("bb_missing_body", FIXTURE),
        "invalid declaration in uselistorder_bb"
    );
}

/// Ports `test/Assembler/invalid-uselistorder_bb-numbered.ll`, whose CHECK
/// line is `error: invalid numeric label in uselistorder_bb`. A `%N` label
/// is rejected on sight — the directive takes names only.
#[test]
fn uselistorder_bb_rejects_a_numeric_label() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_numbered.ll");

    assert_eq!(
        parse_err("bb_numbered", FIXTURE),
        "invalid numeric label in uselistorder_bb"
    );
}

/// Ports `test/Assembler/invalid-uselistorder_bb-missing-bb.ll`, whose CHECK
/// line is `error: invalid basic block in uselistorder_bb` — the name is
/// well-formed but the function's symbol table has nothing under it.
#[test]
fn uselistorder_bb_rejects_an_unknown_label() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_missing_bb.ll");

    assert_eq!(
        parse_err("bb_missing_bb", FIXTURE),
        "invalid basic block in uselistorder_bb"
    );
}

/// Ports `test/Assembler/invalid-uselistorder_bb-not-bb.ll`, whose CHECK line
/// is `error: expected basic block in uselistorder_bb`.
///
/// Upstream looks the label up in the function's *value* symbol table, which
/// holds arguments too — so `%arg` is found, and rejected by class rather
/// than by absence.
#[test]
fn uselistorder_bb_rejects_an_argument_named_as_a_label() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/invalid_uselistorder_bb_not_bb.ll");

    assert_eq!(
        parse_err("bb_not_bb", FIXTURE),
        "expected basic block in uselistorder_bb"
    );
}

// --------------------------------------------------------------------------
// Positives — parse, apply, and re-derive
// --------------------------------------------------------------------------

/// Ports `test/Assembler/uselistorder_global.ll`, whose `RUN` line is
/// `opt -S -preserve-ll-uselistorder` and whose CHECK block pins
/// `uselistorder ptr @g, { 3, 2, 1, 0 }` at module level, with `CHECK-NOT`
/// forbidding any directive inside either function.
///
/// This is the wave's load-bearing test: the printed shuffle is *derived* by
/// `predictUseListOrder` from the use list the directive itself produced, so
/// reproducing it requires llvmkit's use-list order convention, its sort, and
/// its `orderModule` numbering all to match upstream at once.
#[test]
fn global_use_list_order_round_trips() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/uselistorder/uselistorder_global.ll");

    // Not named after the fixture: `; ModuleID = '<name>'` is printed first,
    // and the `CHECK-NOT` below is a substring search.
    let printed = parse_and_print_preserving("global_round_trip", FIXTURE);
    assert!(
        printed.contains("uselistorder ptr @g, { 3, 2, 1, 0 }\n"),
        "expected the module-level directive to be re-derived, got:\n{printed}"
    );
    // The fixture's `CHECK-NOT`s: neither function body may carry one. The
    // module-level section that follows is introduced by `printUseLists`'s own
    // `; uselistorder directives` comment, so the bodies are everything before
    // that header.
    let (bodies, _) = printed
        .split_once("; uselistorder directives")
        .expect("the module-level section is emitted");
    assert!(
        !bodies.contains("uselistorder"),
        "no directive belongs inside either function, got:\n{printed}"
    );
}

/// Ports `test/Assembler/uselistorder.ll`, whose `RUN` line asserts `llvm-as`
/// emits neither an error nor a warning — every directive in it is accepted,
/// including the `ConstantInt` and `label` operands.
///
/// **llvmkit still rejects this fixture, and not for a use-list reason.** Its
/// second line is `@b = alias i1, getelementptr ([4 x i1], ptr @a, i64 0, i64 2)`
/// — an aliasee written as a leading constant expression with no type of its
/// own. `parse_constant_expr` takes a `result_ty` and llvmkit has no
/// self-typing entry point, so the `getelementptr` is met with
/// `expected type`. That gap is recorded as D6 in `docs/divergences.md`; it
/// is `parseValID`'s type-agnostic refactor applied one level down, and it is
/// what upstream's `invalid aliasee` is reached through.
///
/// The assertion below therefore pins **llvmkit's current answer, not
/// upstream's**, so that closing D6 makes this test fail and demands the port
/// be finished. Everything the wave itself owns is already covered: the
/// `label` operands by
/// [`a_block_after_a_directive_is_rejected`], the `ConstantInt` operand by the
/// same, and the module-level directives by
/// [`global_use_list_order_round_trips`].
#[test]
fn the_upstream_uselistorder_fixture_is_blocked_on_the_self_typed_aliasee() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/uselistorder/uselistorder.ll");

    assert_eq!(parse_err("uselistorder_full", FIXTURE), "expected type");
}

/// Ports `test/Assembler/uselistorder_bb.ll`, whose `RUN` line likewise
/// asserts a clean `llvm-as`. Its three `uselistorder_bb` directives name
/// blocks reached only through `blockaddress` constants in *other*
/// functions and in globals.
#[test]
fn the_upstream_uselistorder_bb_fixture_parses_clean() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/uselistorder/uselistorder_bb.ll");

    let module = Module::dynamic("uselistorder_bb");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");
}

/// Ports `test/Assembler/function-operand-uselistorder.ll`, a
/// `verify-uselistorder` fixture whose point is that a `Function`'s own
/// operands — prefix, prologue and personality — are `Use` edges like any
/// other, so `@g` is used six times across two functions.
///
/// llvmkit models those six as [`ValueUse::GlobalField`] edges rather than
/// as `User` operands, which is what makes them countable here at all.
#[test]
fn function_operands_participate_in_the_use_list() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/function_operand_uselistorder.ll");

    let module = Module::dynamic("function_operand_uselistorder");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");

    let g = module.global("g").expect("@g is defined");
    assert_eq!(module.view(g).as_erased().num_uses(), 6);
}

/// A module printed *without* the preserve option carries no directives,
/// mirroring `Module::print`'s `ShouldPreserveUseListOrder = false` default —
/// which is what `llvm-dis` passes and therefore what `Display` must do.
///
/// llvmkit-specific: upstream has no single fixture pinning the default,
/// because its `.ll` tests select the behaviour through `llvm-dis` versus
/// `opt -preserve-ll-uselistorder` on the `RUN` line instead.
#[test]
fn plain_display_emits_no_directives() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/uselistorder/uselistorder_global.ll");

    // Deliberately not named after the fixture: the module identifier is
    // printed as `; ModuleID = '<name>'`, so a name containing the word would
    // make the assertion below vacuously interesting.
    let module = Module::dynamic("default_print");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");

    assert!(!format!("{module}").contains("uselistorder"));
}
