//! Inputs whose cost grew faster than their size: each test here parses or
//! queries a small or ordinary input that some routine used to handle in
//! time quadratic in the input, or proportional to a declared size, and
//! pins the bound that replaced it.
//!
//! **No upstream counterpart.** Upstream has no timing tests; each test names
//! the upstream routine whose cost it holds llvmkit to. A regression shows up
//! as the deadline in [`support::deadline::within_a_minute`] exiting the
//! binary, and, where a count can say more than a clock, as a failed count.

use llvmkit_asmparser::parser;
use llvmkit_ir::{DataLayout, DynBrand, IrError, Module, SizedType, Unverified};

pub mod support;

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

/// One value given 2^20 uses: `%p` is the address of 2^20 stores.
///
/// **No upstream counterpart.** Upstream's `Use::addToList` (`Use.h`) is a
/// constant-time prepend, and llvmkit's `Context::add_use` matches it with a
/// deque's `push_front`. When it shifted the whole list instead
/// (`Vec::insert(0, ..)`), a value with K uses cost K squared to build: 80,000
/// stores took 6.07 s to parse at `34fdec9` (wall time of
/// `target/release/examples/parse_file` on a file of 80,000
/// `store i32 0, ptr %p`, rustc 1.96.0), so 2^20, 13 times as many, would
/// take about 170 times as long, past the deadline. The count and the head of the list are the
/// order upstream's list has: every store, newest first.
#[test]
fn a_value_with_a_million_uses_is_built_in_linear_time() {
    support::deadline::within_a_minute("adding a use shifts the use list", || {
        const STORES: usize = 1 << 20;
        let mut source = String::from("define void @f(ptr %p) {\n");
        for _ in 0..STORES {
            source.push_str("  store i32 0, ptr %p\n");
        }
        source.push_str("  ret void\n}\n");
        let module = parse(&source);
        let function = module.function_dyn("f").expect("@f is defined");
        let pointer = module
            .view(function)
            .param(0)
            .expect("@f has %p")
            .as_erased();
        assert_eq!(pointer.num_uses(), STORES);
        let newest = pointer.users().next().expect("%p has users");
        let last_store = module
            .view(function)
            .entry_block()
            .expect("@f has a body")
            .instructions()
            .nth(STORES - 1)
            .expect("the last store");
        assert_eq!(newest, last_store, "the newest use heads the list");
    });
}

/// `-1` at the widest integer type, `i8388608` (`IntegerType::MAX_INT_BITS`,
/// 2^23), parses and prints back.
///
/// **No upstream counterpart.** Upstream's `APInt::setBit` (`APInt.h`) writes
/// one word, and `APInt::sext` (`APInt.cpp`) is a copy and a fill, so a wide
/// `-1` costs its words. llvmkit's `ApInt::set_bit` copied every word to set
/// one, and `ApInt::bits_set` set its range a bit at a time, so sign-extending
/// the literal to its type cost the width squared over 64: `i65536 -1` took
/// 0.05 s to parse and `i262144 -1` 0.76 s at `34fdec9` (wall time of
/// `target/release/examples/parse_file` on the one-line file, rustc 1.96.0),
/// and this width is 32 times the second, so about 1,000 times as long.
#[test]
fn the_widest_integer_literal_is_built_in_linear_time() {
    support::deadline::within_a_minute("setting a bit copies the whole integer", || {
        let module = parse("@a = global i8388608 -1\n");
        let printed = format!("{module}");
        assert!(
            printed.contains("@a = global i8388608 -1\n"),
            "the literal prints back:\n{printed}"
        );
    });
}

/// A function of 200,000 blocks, each defined where it is first named.
///
/// **No upstream counterpart.** Upstream resolves a label with
/// `dyn_cast<BasicBlock>` on the value it looked up, and moves a block it
/// defines to the end with `Function::splice` on an intrusive list, both
/// constant time. llvmkit's parser found the block of a label value by
/// searching every block of the function (`block_view_for_value`), and
/// `FunctionValue::move_basic_block_to_end` searched for the block from the
/// front, so each block cost a walk of the blocks before it: 80,000 blocks
/// took 5.90 s to parse at `34fdec9` (wall time of
/// `target/release/examples/parse_file` on a file of 80,000
/// `bN:\n  ret void` blocks, rustc 1.96.0). The label is now
/// narrowed by `FunctionValue::basic_block`, and the move searches from the
/// back, where a block defined as it is appended already sits.
#[test]
fn a_function_of_two_hundred_thousand_blocks_parses_in_linear_time() {
    support::deadline::within_a_minute("each block walks the blocks before it", || {
        const BLOCKS: usize = 200_000;
        let mut source = String::from("define void @f() {\n  ret void\n");
        for index in 0..BLOCKS {
            source.push_str(&format!("b{index}:\n  ret void\n"));
        }
        source.push_str("}\n");
        let module = parse(&source);
        let function = module.function_dyn("f").expect("@f is defined");
        let blocks: Vec<_> = module.view(function).basic_blocks().collect();
        assert_eq!(blocks.len(), BLOCKS + 1);
        assert_eq!(
            blocks.last().and_then(|block| block.name()).as_deref(),
            Some(format!("b{}", BLOCKS - 1).as_str()),
            "blocks stay in definition order"
        );
    });
}

/// A summary whose global-value ids are near `u32::MAX`, one referenced
/// before and one after its definition.
///
/// **No upstream counterpart.** Upstream's `LLParser::addGlobalValueToIndex`
/// stores `^N` at index `N` of `NumberedValueInfos`, resizing the vector to
/// `N + 1`, so a 78-byte summary naming `^4000000000` asks for 64 GB (16
/// bytes a slot); llvmkit's parser did the same and aborted on the
/// allocation. The parser now keys the ids in a map. The non-contiguous ids
/// upstream does test are `parser_summary.rs::index_value_order_renumbers_summary_ids`.
/// What this pins beyond the deadline: both references resolve, to the GUIDs
/// the ids were defined with, and the printer renumbers the ids densely.
#[test]
fn a_summary_id_near_the_top_of_the_range_costs_one_entry() {
    support::deadline::within_a_minute("a summary id sizes a vector", || {
        let index = llvmkit_asmparser::parse_summary_index_assembly(concat!(
            "^0 = module: (path: \"a.o\", hash: (0, 0, 0, 0, 0))\n",
            "^4000000000 = gv: (guid: 1)\n",
            "^4000000001 = gv: (guid: 2, summaries: (variable: (module: ^0, ",
            "flags: (linkage: external), varFlags: (readonly: 0), ",
            "refs: (^4000000000, ^4000000002))))\n",
            "^4000000002 = gv: (guid: 3)\n",
        ))
        .expect("the index parses");
        let printed = index.to_string();
        assert!(
            printed.contains("refs: (^1, ^3)"),
            "both references resolve:\n{printed}"
        );
    });
}

/// The alloc size of a struct 40 levels deep, each level two copies of the
/// one below: `%tK = type { %t(K-1), %t(K-1) }` over `%t0 = type { i8, i16 }`.
///
/// **No upstream counterpart.** Upstream's `DataLayout::getStructLayout`
/// (`DataLayout.cpp`) caches each struct's layout in its `StructLayoutMap`,
/// so each of the 41 structs is laid out once. llvmkit laid a struct out
/// again every time a walk reached it, twice for each field (`alignment` and
/// `type_alloc_size_inner`, read at `34fdec9`), so laying out `%tK` laid out
/// `%t(K-1)` four times, and this query would lay `%t0` out 4^40 times. A
/// `DataLayout` now keeps each struct's layout, as upstream's does
/// (`StructLayoutMap`).
///
/// The expected size is derived by hand, not taken from upstream: `%t0` is
/// 4 bytes (`i8` at 0, `i16` at 2), and each level is two copies of the level
/// below with no padding (its 2-byte alignment divides its size), so `%tK` is
/// `4 * 2^K` bytes.
#[test]
fn a_struct_shared_forty_levels_deep_is_laid_out_once_per_level() {
    support::deadline::within_a_minute("a shared struct is laid out on every visit", || {
        const LEVELS: u32 = 40;
        let mut source = String::from("%t0 = type { i8, i16 }\n");
        for level in 1..=LEVELS {
            let below = level - 1;
            source.push_str(&format!("%t{level} = type {{ %t{below}, %t{below} }}\n"));
        }
        source.push_str(&format!("@g = external global %t{LEVELS}\n"));
        let module = parse(&source);
        let global = module.global("g").expect("@g is declared");
        let ty =
            SizedType::try_from(module.view(global).value_type()).expect("the struct is sized");
        let size = DataLayout::default().type_alloc_size(ty);
        assert!(!size.is_scalable());
        assert_eq!(size.known_min_value(), 4u64 << LEVELS);
    });
}

/// A struct 64 levels deep, each level two copies of the one below, that is
/// allocated, indexed, loaded and stored, and given a zero initializer.
///
/// **No upstream counterpart.** The type is `4 * 2^64` bytes, past `u64`, so
/// its size saturates at `u64::MAX` (`docs/divergences.md` D42), which the
/// test asserts; before the saturation was made consistent, `align_to`
/// overflowed and panicked here in a debug build. Before
/// `struct_is_sized` remembered a sized struct (`SCDB_IsSized`), the parser
/// refused such a type as unsized before any walk reached it. Remembering
/// makes it reachable, and every walk the parser, verifier and printer run
/// over it has to cost the type's distinct structs, not its 2^64 paths:
/// `isSized`, `isScalableTy` and `getStructLayout` each memoise per struct
/// upstream, and so must these.
#[test]
fn a_struct_shared_sixty_four_levels_deep_parses_verifies_and_prints() {
    support::deadline::within_a_minute("a walk visits a shared struct once per path", || {
        const LEVELS: u32 = 64;
        let mut source = String::from("%t0 = type { i8, i16 }\n");
        for level in 1..=LEVELS {
            let below = level - 1;
            source.push_str(&format!("%t{level} = type {{ %t{below}, %t{below} }}\n"));
        }
        let top = format!("%t{LEVELS}");
        source.push_str(&format!("@g = global {top} zeroinitializer\n"));
        source.push_str(&format!(
            "define void @f(ptr %p) {{\n  %a = alloca {top}\n  \
             %q = getelementptr {top}, ptr %p, i64 1, i32 1\n  \
             %v = load {top}, ptr %p\n  \
             store {top} zeroinitializer, ptr %a\n  ret void\n}}\n"
        ));
        let module = parse(&source);
        // 4 * 2^64 bytes has no `u64`: both byte sizes saturate (D42),
        // where upstream's arithmetic wraps, and asking does not panic.
        let global = module.global("g").expect("@g is defined");
        let ty =
            SizedType::try_from(module.view(global).value_type()).expect("the struct is sized");
        let data_layout = DataLayout::default();
        assert_eq!(data_layout.type_alloc_size(ty).known_min_value(), u64::MAX);
        assert_eq!(data_layout.type_store_size(ty).known_min_value(), u64::MAX);
        let printed = format!("{module}");
        parse(&printed).verify().expect("the module verifies");
        assert_eq!(format!("{}", parse(&printed)), printed);
    });
}

/// A size that fits in bytes but not in bits: `%t60` of the same shape is
/// `4 * 2^60` = 2^62 bytes, which is 2^65 bits.
///
/// **No upstream counterpart** (`docs/divergences.md` D42). Upstream's sizes
/// wrap, so its bit size of this type is 0. llvmkit's saturates at
/// `u64::MAX` bits, and its byte sizes stay exact: the store size is read in
/// bytes for an aggregate, not divided down from the saturated bit count,
/// which would answer 2^61 - 1 bytes, a plausible size for a type eight times
/// larger. The expected 2^62 is derived by hand as in
/// [`a_struct_shared_forty_levels_deep_is_laid_out_once_per_level`].
#[test]
fn a_size_past_sixty_four_bits_saturates_without_shrinking_its_bytes() {
    const LEVELS: u32 = 60;
    let mut source = String::from("%t0 = type { i8, i16 }\n");
    for level in 1..=LEVELS {
        let below = level - 1;
        source.push_str(&format!("%t{level} = type {{ %t{below}, %t{below} }}\n"));
    }
    source.push_str(&format!("@g = external global %t{LEVELS}\n"));
    let module = parse(&source);
    let global = module.global("g").expect("@g is declared");
    let ty = SizedType::try_from(module.view(global).value_type()).expect("the struct is sized");
    let data_layout = DataLayout::default();
    assert_eq!(
        data_layout.type_alloc_size(ty).known_min_value(),
        4u64 << LEVELS
    );
    assert_eq!(
        data_layout.type_store_size(ty).known_min_value(),
        4u64 << LEVELS
    );
    assert_eq!(
        data_layout.type_size_in_bits(ty).known_min_value(),
        u64::MAX
    );
}

/// One struct of 20,000 fields, each its own struct type, whose alignment
/// is asked 20,000 times under each of five `DataLayout`s, taking turns.
///
/// **No upstream counterpart.** Upstream's `DataLayout::getStructLayout`
/// keeps each struct's layout in the `DataLayout`'s `StructLayoutMap`, so
/// only the first query of each `DataLayout` lays the struct out. llvmkit's
/// `DataLayout` keeps its map the same way (`StructLayoutMap`). A memo that
/// lived for one query, a cache on the module that kept one `DataLayout`'s
/// layouts and dropped them when another asked, or one that kept the four
/// most recent, laid out all 20,001 structs on every query: 100,000 queries
/// of 40,000 field visits each.
#[test]
fn a_struct_asked_about_again_is_not_laid_out_again() {
    support::deadline::within_a_minute("a struct is laid out again on every query", || {
        const FIELDS: usize = 20_000;
        const QUERIES: usize = 20_000;
        let mut source = String::new();
        for field in 0..FIELDS {
            source.push_str(&format!("%s{field} = type {{ i8 }}\n"));
        }
        let fields: Vec<String> = (0..FIELDS).map(|field| format!("%s{field}")).collect();
        source.push_str(&format!("%top = type {{ {} }}\n", fields.join(", ")));
        source.push_str("@g = external global %top\n");
        let module = parse(&source);
        let global = module.global("g").expect("@g is declared");
        let ty =
            SizedType::try_from(module.view(global).value_type()).expect("the struct is sized");
        let layouts: Vec<(DataLayout, u64)> = [1u64, 2, 4, 8, 16]
            .into_iter()
            .map(|bytes| {
                let spec = format!("e-a:{}", bytes * 8);
                (DataLayout::parse(spec).expect("a valid layout"), bytes)
            })
            .collect();
        for _ in 0..QUERIES {
            for (layout, bytes) in &layouts {
                assert_eq!(layout.abi_type_align(ty).value(), *bytes);
            }
        }
    });
}

/// One struct of 100,000 `i8` fields whose layout is asked 1,000,000 times.
///
/// **No upstream counterpart.** Upstream's `DataLayout::getStructLayout`
/// answers a struct it has laid out from its `StructLayoutMap` before
/// anything else, so a repeated query costs one lookup. At `f54f4825`
/// llvmkit's `struct_layout` scanned every field for sizedness before that
/// lookup, so every query cost the struct's fields again: here 10^11 field
/// checks. With 100,000 queries (10^10 checks) and that order put back on
/// top of `a5fecaaa`, this test ran past its deadline in the gate's debug
/// profile (`cargo +1.96.0 test -p llvmkit-asmparser --test
/// resource_bounds -- a_kept`); ten times as many leaves a fast machine no
/// way through. The lookup now comes first, and the layout walk itself
/// checks each field as it reaches it.
#[test]
fn a_kept_struct_layout_is_answered_without_rechecking_its_fields() {
    support::deadline::within_a_minute("a kept layout rechecks every field", || {
        const FIELDS: usize = 100_000;
        const QUERIES: usize = 1_000_000;
        let source = format!("%top = type {{ {} }}\n", vec!["i8"; FIELDS].join(", "));
        let module = parse(&source);
        let top = module.named_struct("top").expect("%top is defined");
        let data_layout = DataLayout::default();
        for _ in 0..QUERIES {
            let layout = data_layout
                .struct_layout(top)
                .expect("every field is sized");
            assert_eq!(layout.size_in_bytes().fixed_value(), Some(100_000));
        }
    });
}

/// Structs of 100,000 `i8` fields and one last field upstream asserts on,
/// each struct's layout asked 100,000 times.
///
/// **No upstream counterpart.** Upstream asserts on each of these layouts,
/// and llvmkit refuses them (`docs/divergences.md` D43):
/// - a scalable vector last (`TypeSize::operator+=`'s "Incompatible types"
///   when its scalable size meets the fixed 100,000 bytes before it);
/// - `{ <vscale x 4 x i32>, <vscale x 8 x i16> }` last, which is unsized
///   without reaching an opaque struct (`getABITypeAlign`'s `isSized`);
/// - in a packed struct, `{ <vscale x 4 x i32>, i8 }` last, a struct refused
///   for an incompatible sum, so `getTypeAllocSize` asserts on it.
///
/// Each refusal comes only after every field before the last is measured,
/// and none can change, since a body is set once and none of these reaches
/// an opaque struct; so each is kept in the struct-layout map beside the
/// layouts. Before `e4baafdc` none was kept; at `e4baafdc` only the first
/// was, and a query of either of the other two measured the 100,000 fields
/// again.
#[test]
fn a_refused_struct_layout_is_kept_as_refused() {
    support::deadline::within_a_minute("a refused layout is measured again", || {
        const FIELDS: usize = 100_000;
        const QUERIES: usize = 100_000;
        let fixed = vec!["i8"; FIELDS].join(", ");
        let source = format!(
            "%scalable_last = type {{ {fixed}, <vscale x 4 x i32> }}\n\
             %unsized_last = type {{ {fixed}, {{ <vscale x 4 x i32>, <vscale x 8 x i16> }} }}\n\
             %packed_refused_last = type <{{ {fixed}, {{ <vscale x 4 x i32>, i8 }} }}>\n"
        );
        let module = parse(&source);
        let data_layout = DataLayout::default();
        for name in ["scalable_last", "unsized_last", "packed_refused_last"] {
            let ty = module.named_struct(name).expect("the struct is defined");
            for _ in 0..QUERIES {
                assert!(data_layout.struct_layout(ty).is_none(), "%{name}");
            }
        }
    });
}

/// A struct 30 levels deep, each level two copies of the one below, over a
/// struct that holds an opaque struct.
///
/// **No upstream counterpart.** Upstream asserts `isSized` on every size
/// query of such a type. llvmkit's size queries take a [`SizedType`] proof,
/// which this type refuses (`IrError::UnsizedType`), and `struct_layout`
/// answers `None` for it. Both ask `isSized`, whose walk stops at the first
/// unsized field, as upstream's `StructType::isSized` does. Before the
/// queries took the proof they laid such a struct out with the opaque field
/// empty, and a layout that was not kept was redone on every visit of a
/// shared struct, twice per field: this query would have laid `%t0` out
/// 4^30 times.
#[test]
fn a_shared_struct_over_an_opaque_field_is_refused_at_once() {
    support::deadline::within_a_minute("an unsized struct is walked on every path", || {
        const LEVELS: u32 = 30;
        let mut source = String::from("%O = type opaque\n%t0 = type { %O }\n");
        for level in 1..=LEVELS {
            let below = level - 1;
            source.push_str(&format!("%t{level} = type {{ %t{below}, %t{below} }}\n"));
        }
        source.push_str(&format!("@g = external global %t{LEVELS}\n"));
        let module = parse(&source);
        let global = module.global("g").expect("@g is declared");
        let ty = module.view(global).value_type();
        assert!(
            matches!(SizedType::try_from(ty), Err(IrError::UnsizedType { .. })),
            "an unsized type has no size proof"
        );
        let data_layout = DataLayout::default();
        let top = module
            .named_struct(&format!("t{LEVELS}"))
            .expect("the top struct is defined");
        assert!(
            data_layout.struct_layout(top).is_none(),
            "it holds an opaque struct"
        );
    });
}

/// A named struct of 100,000 fields, each the same struct `%s`, which reaches
/// 1,000 distinct structs of its own.
///
/// **No upstream counterpart.** Upstream's `StructType::checkBody`
/// (`Type.cpp`), which `setBody` runs to refuse a recursive body, walks one
/// `SmallSetVector` worklist of every type the body reaches, entering each
/// once, so this body costs its 1,002 distinct types. llvmkit walked from
/// each field with a fresh visited list, searched linearly, so each of the
/// 100,000 fields walked `%s`'s reach again at a cost quadratic in it: about
/// 5 * 10^10 comparisons. `Context::body_would_be_recursive` is now the
/// worklist.
#[test]
fn a_wide_body_over_a_shared_struct_is_checked_once() {
    support::deadline::within_a_minute("a body's check walks each field's reach", || {
        const REACH: usize = 1_000;
        const FIELDS: usize = 100_000;
        let mut source = String::new();
        for index in 0..REACH {
            source.push_str(&format!("%d{index} = type {{ i8 }}\n"));
        }
        let reached: Vec<String> = (0..REACH).map(|index| format!("%d{index}")).collect();
        source.push_str(&format!("%s = type {{ {} }}\n", reached.join(", ")));
        source.push_str(&format!(
            "%top = type {{ {} }}\n",
            vec!["%s"; FIELDS].join(", ")
        ));
        let module = parse(&source);
        let top = module.named_struct("top").expect("%top is defined");
        assert_eq!(top.field_count(), FIELDS, "every field is in the body");
    });
}

/// A named struct of 200,000 fields, each its own struct type, given a zero
/// initializer, which asks whether it is sized.
///
/// **No upstream counterpart.** Upstream's `StructType::isSized` (`Type.cpp`)
/// threads a `SmallPtrSet` of the structs it has visited, so asking costs the
/// distinct structs reached. llvmkit's visited set was a list searched on
/// every struct, so each of the 200,000 fields searched the fields before
/// it: about 2 * 10^10 comparisons. `is_sized` and `is_scalable` now thread a
/// set.
#[test]
fn a_wide_struct_of_distinct_structs_is_sized_in_linear_time() {
    support::deadline::within_a_minute("isSized searches a list of visited structs", || {
        const FIELDS: usize = 200_000;
        let mut source = String::new();
        for field in 0..FIELDS {
            source.push_str(&format!("%s{field} = type {{ i8 }}\n"));
        }
        let fields: Vec<String> = (0..FIELDS).map(|field| format!("%s{field}")).collect();
        source.push_str(&format!("%top = type {{ {} }}\n", fields.join(", ")));
        source.push_str("@g = global %top zeroinitializer\n");
        let module = parse(&source);
        let global = module.global("g").expect("@g is defined");
        let ty = module.view(global).value_type();
        assert!(ty.is_sized(), "a struct of sized structs is sized");
    });
}
