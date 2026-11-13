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
use llvmkit_ir::{DynBrand, Module, Unverified};

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
