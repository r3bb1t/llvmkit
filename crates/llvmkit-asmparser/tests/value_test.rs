//! Ports of `llvm/unittests/IR/ValueTest.cpp` that need the `.ll` parser.

use llvmkit_asmparser::parser;

/// Port of `unittests/IR/ValueTest.cpp::TEST(ValueTest, setNameShrink)`,
/// whole: parse a module, take the function `@f1`, drop the last character of
/// its name, rename it to that, and read the name back.
#[test]
fn set_name_shrink() {
    let module_string = concat!("define void @f1() {\n", "bb0:\n", "  ret void\n", "}\n");
    let m = parser::parse_dynamic(module_string).expect("the module parses");

    let f = m.view(m.function_dyn("f1").expect("@f1 is declared"));
    let mut name = f.name().unwrap_or_default();
    name.pop();
    f.set_name(&m, name)
        .expect("the shortened name is accepted");
    assert_eq!(f.name().unwrap_or_default(), "f");
}
