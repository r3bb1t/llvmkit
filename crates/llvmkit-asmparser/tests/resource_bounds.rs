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
use llvmkit_ir::{DataLayout, DynBrand, Module, Unverified};

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
/// stores took 5.2 s to parse at `34fdec9` (release build), so 2^20 would take
/// minutes, past the deadline. The count and the head of the list are the
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
/// the literal to its type cost the width squared over 64: `i1048576 -1` took
/// 8.1 s to parse at `34fdec9` (release build), and the curve puts this width
/// at about nine minutes.
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
/// took 5.9 s to parse at `34fdec9` (release build). The label is now
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
/// so the query lays out each of the 41 structs once. llvmkit laid a struct
/// out again every time a walk reached it, twice for each field
/// (`alignment` and `type_alloc_size_inner`), four times per level: 14
/// levels took 18.6 s at `34fdec9` (release build), so 40 would take years.
/// Each query now memoises the structs it lays out.
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
        let size = DataLayout::default().type_alloc_size(module.view(global).value_type());
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
        // 4 * 2^64 bytes has no `u64`: the size saturates (D42), where
        // upstream's arithmetic wraps, and asking for it does not panic.
        let global = module.global("g").expect("@g is defined");
        let size = DataLayout::default().type_alloc_size(module.view(global).value_type());
        assert_eq!(size.known_min_value(), u64::MAX);
        let printed = format!("{module}");
        parse(&printed).verify().expect("the module verifies");
        assert_eq!(format!("{}", parse(&printed)), printed);
    });
}
