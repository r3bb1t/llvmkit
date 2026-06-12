//! Constant parser tests.
//!
//! Mirrors upstream aggregate-constant parsing from
//! `test/Assembler/aggregate-constant-values.ll`.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::Module;

fn parse_and_render(src: &str) -> String {
    let module = Module::new("parser_constants");
    Parser::new(src.as_bytes(), &module)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
    format!("{module}")
}

/// Mirrors `test/Assembler/aggregate-constant-values.ll`: array constant
/// initializer syntax (`[i32 1, i32 2]`) as a global initializer.
#[test]
fn array_constant_initializer_round_trips() {
    let text = parse_and_render("@arr = global [2 x i32] [i32 1, i32 2]\n");
    assert!(
        text.contains("@arr = global [2 x i32] [i32 1, i32 2]"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/aggregate-constant-values.ll`: struct constant
/// initializer syntax (`{ i32 1, i32 2 }`) as a global initializer.
#[test]
fn struct_constant_initializer_round_trips() {
    let text = parse_and_render("@pair = global { i32, i32 } { i32 1, i32 2 }\n");
    assert!(
        text.contains("@pair = global { i32, i32 } { i32 1, i32 2 }"),
        "AsmWriter output: {text}"
    );
}

/// Mirrors `test/Assembler/getelementptr.ll`: a global initializer may be a
/// `getelementptr` constant expression.
#[test]
fn getelementptr_constant_expr_initializer_round_trips() {
    let text = parse_and_render(
        "@data = global i8 0\n@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)\n",
    );
    assert!(
        text.contains("@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)"),
        "AsmWriter output: {text}"
    );
}
