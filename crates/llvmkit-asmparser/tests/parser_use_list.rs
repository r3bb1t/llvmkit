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
/// Its second line, `@b = alias i1, getelementptr ([4 x i1], ptr @a, i64 0, i64 2)`,
/// is an aliasee written as a leading constant expression with no type of its
/// own — `LLParser::parseAliasOrIFunc`'s bare-`parseValID` branch. That is what
/// blocked this fixture until `parse_constant_expr` became self-typing.
#[test]
fn the_upstream_uselistorder_fixture_parses_clean() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/uselistorder/uselistorder.ll");

    let module = Module::dynamic("uselistorder_full");
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");
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

// --------------------------------------------------------------------------
// A global's uses are one list
// --------------------------------------------------------------------------

/// Parse `printed` into a fresh module named `module_name`, and print that
/// with use-list order preserved: the assembly half of `verify-uselistorder`'s
/// `verifyUseListOrder` (`tools/verify-uselistorder/verify-uselistorder.cpp`),
/// which writes `M.print(OS, nullptr, /*ShouldPreserveUseListOrder=*/true)`
/// and reads it back with `parseAssemblyFile`.
///
/// Upstream then compares the two modules' use lists, user by user and
/// operand by operand (`matches`). llvmkit has no public view of a use list's
/// order, so a caller compares the two prints instead. Those agree exactly
/// when the orders do wherever the printer can tell two orders apart, which
/// is everywhere but between two uses by one instruction or one constant
/// (`docs/divergences.md` D4); each caller says why its case is not that one.
fn reprint_preserving(module_name: &str, printed: &str) -> String {
    parse_and_print_preserving(module_name, printed.as_bytes())
}

/// Ports the `verify-uselistorder` RUN line of
/// `test/Assembler/function-operand-uselistorder.ll`, for the steps that tool
/// takes deterministically: `verifyUseListOrder` on the order as parsed, then
/// `reverseUseLists` and `verifyUseListOrder` again. Its `NumShuffles` random
/// shuffles are not ported: they draw from `std::minstd_rand0` through
/// `std::uniform_int_distribution<short>`, whose mapping the C++ standard
/// leaves to the library. Only the assembly half of `verifyUseListOrder` runs,
/// since llvmkit has no bitcode.
///
/// `@g` is the only value here with two or more uses, so reversing its list is
/// all `reverseUseLists` does. Its six uses are the personality, prefix and
/// prologue of two functions, so only `getOperandNo` orders the three of one
/// function, and the printer reads it off the field each edge names. The
/// printed shuffle, `{ 5, 4, 3, 2, 1, 0 }`, is derived from
/// `predictValueUseListOrder`, not run. `uselistorder.ll`, the other fixture a
/// function's `uselistorder` might come from, states no directive on a
/// function.
#[test]
fn function_operand_uses_survive_a_reversal_round_trip() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/uselistorder/function_operand_uselistorder.ll");
    const NAME: &str = "function_operand_round_trip";

    let module = Module::dynamic(NAME);
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");

    // As parsed, the order is the one a parse rebuilds: nothing to state.
    let printed = module.preserving_use_list_order().to_string();
    assert!(
        !printed.contains("uselistorder"),
        "the parsed order needs no directive, got:\n{printed}"
    );
    assert_eq!(reprint_preserving(NAME, &printed), printed);

    // `reverseUseLists`.
    let g = module.global("g").expect("@g is defined");
    module
        .view(g)
        .as_erased()
        .sort_use_list(&[5, 4, 3, 2, 1, 0])
        .expect("@g has six uses");
    let printed = module.preserving_use_list_order().to_string();
    assert!(
        printed.contains("\nuselistorder ptr @g, { 5, 4, 3, 2, 1, 0 }\n"),
        "expected the reversal to be stated, got:\n{printed}"
    );
    assert_eq!(reprint_preserving(NAME, &printed), printed);
}

/// llvmkit-specific: a function's uses are one list, whatever names it, as
/// upstream's `Function` has one `UseList` — a direct call, a `ptr @f` store
/// operand, a global's initializer and a `getelementptr` constant, the callee
/// known when the call is read or forward-referenced.
///
/// Before, the direct call and the initializer registered on the function,
/// and the store, the expression and a forward-referenced call on the
/// interned `ptr @f` wrapper (`docs/divergences.md` D3), so `uselistorder` saw
/// half the uses or none: `value has no uses` for the known callee called
/// twice, `wrong number of indexes, expected 2` for the four uses below, and a
/// forward-referenced callee's directive was accepted and then never printed.
/// No `test/Assembler` fixture states a directive on a function; the upstream
/// ones are inputs to `opt`, as in
/// [`a_directive_on_an_invoked_function_parses`]. Each printed shuffle is
/// derived from `predictValueUseListOrder`, not run, and the round trip is
/// `verify-uselistorder`'s, as [`reprint_preserving`] says; no two uses here
/// share a user.
#[test]
fn a_function_called_and_named_by_constants_orders_every_use() {
    const CASES: [(&str, &str, &str); 3] = [
        (
            "known_callee",
            "declare void @f()\n\
             define void @g() {\n  call void @f()\n  call void @f()\n  ret void\n}\n\
             uselistorder ptr @f, { 1, 0 }\n",
            "\nuselistorder ptr @f, { 1, 0 }\n",
        ),
        (
            "forward_callee",
            "define void @g() {\n  call void @f()\n  call void @f()\n  ret void\n}\n\
             declare void @f()\n\
             uselistorder ptr @f, { 1, 0 }\n",
            "\nuselistorder ptr @f, { 1, 0 }\n",
        ),
        (
            "called_and_named_by_constants",
            "declare void @f()\n\
             @k = global ptr @f\n\
             @e = global ptr getelementptr (i8, ptr @f, i64 1)\n\
             define void @g() {\n  call void @f()\n  store ptr @f, ptr @k\n  ret void\n}\n\
             uselistorder ptr @f, { 3, 2, 1, 0 }\n",
            "\nuselistorder ptr @f, { 3, 2, 0, 1 }\n",
        ),
    ];
    for (name, source, directive) in CASES {
        let printed = parse_and_print_preserving(name, source.as_bytes());
        assert!(
            printed.contains(directive),
            "{name}: expected `{}`, got:\n{printed}",
            directive.trim()
        );
        assert_eq!(reprint_preserving(name, &printed), printed, "{name}");
    }
}

/// Ports the `verify-uselistorder` RUN line of
/// `test/Assembler/alias-use-list-order.ll` the way
/// [`function_operand_uses_survive_a_reversal_round_trip`] ports its fixture's:
/// the parsed order, then `reverseUseLists`, each through the assembly round
/// trip, the shuffles and the bitcode half left out for the reasons given
/// there.
///
/// The value with two or more uses is the uniqued
/// `getelementptr inbounds (i32, ptr @alias, i64 1)`, named by two
/// initializers ahead of `@alias` and two aliasees after it. Resolving the
/// forward reference re-interns it, and llvmkit used to hand its users over the
/// way `replaceAllUsesWith` does, reversed. Upstream's
/// `ConstantUniqueMap::replaceOperandsInPlace` updates the constant where it
/// stands, so its users keep their order, and the parsed order is the one
/// `predictValueUseListOrder` predicts: llvmkit printed a directive for it, and
/// the re-parse applied that directive to the order it predicted, so the round
/// trip changed the order. The reversed order's shuffle, `{ 3, 2, 1, 0 }`, is
/// derived, not run. No two uses here share a user.
#[test]
fn a_forward_referenced_constant_keeps_its_users_order() {
    const FIXTURE: &[u8] =
        include_bytes!("fixtures/upstream/assembler-corpus/alias-use-list-order.ll");
    const NAME: &str = "alias_use_list_order_round_trip";

    let module = Module::dynamic(NAME);
    Parser::new(FIXTURE, &module)
        .expect("lexer primes")
        .parse_module()
        .expect("fixture parses");

    let printed = module.preserving_use_list_order().to_string();
    assert!(
        !printed.contains("uselistorder"),
        "the parsed order needs no directive, got:\n{printed}"
    );
    assert_eq!(reprint_preserving(NAME, &printed), printed);

    // `reverseUseLists`: the expression is the only value with two uses.
    let reference = module.global("alias.ref1").expect("@alias.ref1 is defined");
    module
        .view(reference)
        .initializer()
        .expect("@alias.ref1 has an initializer")
        .as_erased()
        .sort_use_list(&[3, 2, 1, 0])
        .expect("the expression has four uses");
    let printed = module.preserving_use_list_order().to_string();
    assert!(
        printed.contains(
            "\nuselistorder ptr getelementptr inbounds (i32, ptr @alias, i64 1), { 3, 2, 1, 0 }\n"
        ),
        "expected the reversal to be stated, got:\n{printed}"
    );
    assert_eq!(reprint_preserving(NAME, &printed), printed);
}

/// llvmkit-specific: a constant re-interned for a forward reference takes the
/// place the old one held on every other operand's list, as upstream's
/// `replaceOperandsInPlace` leaves its `Use` where it was, and the old one
/// leaves no edge behind.
///
/// `@x`'s `getelementptr` names `@a` and, through a `ptrtoint`, `@b` before
/// `@b` is defined. Resolving `@b` used to intern a second `getelementptr` and
/// register it at the head of `@a`'s list beside the first, so `@a` had three
/// uses where upstream's has two, the initializer of `@y` and the expression,
/// and `uselistorder ptr @a, { 1, 0 }` was refused as `wrong number of indexes,
/// expected 3`. The printed shuffle is derived from `predictValueUseListOrder`,
/// not run; the round trip is `verify-uselistorder`'s, as
/// [`reprint_preserving`] says.
#[test]
fn a_constant_rewritten_for_a_forward_reference_keeps_its_place() {
    const SOURCE: &str = "@a = global i32 0\n\
         @x = global ptr getelementptr (i8, ptr @a, i64 ptrtoint (ptr @b to i64))\n\
         @y = global ptr @a\n\
         @b = global i32 0\n\
         uselistorder ptr @a, { 1, 0 }\n";
    const NAME: &str = "rewritten_constant_place";

    let printed = parse_and_print_preserving(NAME, SOURCE.as_bytes());
    assert!(
        printed.contains("\nuselistorder ptr @a, { 1, 0 }\n"),
        "expected the directive back, got:\n{printed}"
    );
    assert_eq!(reprint_preserving(NAME, &printed), printed);
}

/// Ports the parse half of `test/Transforms/GVNHoist/pr36787.ll`, whose input
/// states `uselistorder ptr @f0, { 1, 0 }` inside `@func`, on a function
/// invoked twice there. `opt` reads the file with the assembly parser before
/// `gvn-hoist` runs; the pass and the fixture's `CHECK` lines are not ported,
/// since llvmkit has no `gvn-hoist`.
///
/// What is asserted is the parser's half: the file parses, so the directive on
/// the function is accepted, and the order it set prints back, at module
/// level as `predictUseListOrder` files a global's directive. The printed
/// shuffle is derived from `predictValueUseListOrder`, not run: `@f0` is
/// declared before `@func`, so both invokes are numbered after it, and the
/// later one is predicted first.
///
/// The file's `catch ptr @g` names a global variable, which
/// `LLParser::parseLandingPad` accepts because a `GlobalValue` is a
/// `Constant`. llvmkit refused it as `clause argument must be a constant`
/// until the clause check asked about the global's own value category too.
#[test]
fn a_directive_on_an_invoked_function_parses() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/upstream/Transforms/GVNHoist/pr36787.ll");

    let printed = parse_and_print_preserving("invoked_function_directive", FIXTURE);
    assert!(
        printed.contains("\n          catch ptr @g\n"),
        "expected the global catch clause back, got:\n{printed}"
    );
    let (_, module_level) = printed
        .rsplit_once("\n}\n")
        .expect("the module prints a function body");
    assert!(
        module_level.contains("\nuselistorder ptr @f0, { 1, 0 }\n"),
        "expected the function's directive at module level, got:\n{printed}"
    );
}
