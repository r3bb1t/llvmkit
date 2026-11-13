//! `FunctionValue::basic_block`: a value as one of a function's blocks.
//!
//! **No upstream counterpart.** The accessor is llvmkit's spelling of the pair
//! `dyn_cast<BasicBlock>(V)` and `BB->getParent() == F` that
//! `LLParser::PerFunctionState` asks of every label it resolves. These tests
//! pin each of its answers: the parser reaches only the first, so a wrong
//! parent or module check would go unseen there.

use llvmkit_asmparser::parser;
use llvmkit_ir::{DynBrand, Module, Unverified};

const SOURCE: &str = concat!(
    "define void @f(i32 %x) {\n",
    "entry:\n  br label %next\n",
    "next:\n  ret void\n",
    "}\n",
    "define void @g() {\n",
    "entry:\n  ret void\n",
    "}\n",
);

fn parse(source: &str) -> Module<DynBrand, Unverified> {
    match parser::parse_dynamic(source) {
        Ok(module) => module,
        Err(error) => panic!("fixture parses: {error:?}"),
    }
}

/// A block of the function answers that block; a block of another function,
/// a value that is no block, and a block of another module answer `None`.
#[test]
fn a_value_is_a_block_only_of_its_own_function_and_module() {
    let module = parse(SOURCE);
    let f = module.view(module.function_dyn("f").expect("@f"));
    let g = module.view(module.function_dyn("g").expect("@g"));

    let next = f.basic_blocks().nth(1).expect("@f has %next");
    let found = f.basic_block(next.to_erased()).expect("%next is @f's");
    assert_eq!(found.name().as_deref(), Some("next"));

    let g_entry = g.entry_block().expect("@g has a body");
    assert!(
        f.basic_block(g_entry.to_erased()).is_none(),
        "another function's block"
    );

    let argument = f.param(0).expect("@f has %x").as_erased();
    assert!(f.basic_block(argument).is_none(), "not a block");

    let other = parse(SOURCE);
    let other_f = other.view(other.function_dyn("f").expect("@f"));
    let other_next = other_f
        .basic_blocks()
        .nth(1)
        .expect("the other @f has %next");
    assert!(
        f.basic_block(other_next.to_erased()).is_none(),
        "another module's block"
    );
}
