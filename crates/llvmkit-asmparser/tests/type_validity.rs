//! Which derived types exist: the `test/Verifier` fixtures that pin the
//! parser's half of each component rule, and a law tying the builder's half
//! to it.
//!
//! LLVM spells every component rule twice — an `assert` in the type's
//! constructor (`FixedVectorType::get`, `ArrayType::get`,
//! `FunctionType::FunctionType`) or no check at all (`StructType::get`), and an
//! error in `LLParser` — and an `assert` is compiled out of a release build.
//! llvmkit's builder once accepted every shape its parser refused, so a
//! module built in Rust could print text no parser reads back.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::parse_type;
use llvmkit_ir::{DynBrand, IrError, Module, Type};

pub mod support;

/// Parses `fixture` and returns the rejection's message with the
/// `(line, column)` it is anchored at — the coordinates `llvm-as` prints.
///
/// Each fixture's `CHECK` pins only the message; the position comes from the
/// `error(Loc, …)` anchor of the upstream routine that raises it. Comparing
/// the text alone let `invalid-label-param.ll` pass while llvmkit raised
/// upstream's message from a different routine at a different token
/// (`docs/divergences.md` entry 145).
fn rejection(fixture: &[u8]) -> (String, (u32, u32)) {
    let m = Module::dynamic("type_validity");
    let error = Parser::new(fixture, &m)
        .expect("lexer primes")
        .parse_module()
        .expect_err("fixture is rejected");
    let start = usize::try_from(error.loc().start).unwrap_or(usize::MAX);
    (error.to_string(), support::line_and_column(fixture, start))
}

/// Ports `test/Verifier/x86_amx1.ll`: `llvm-as` rejects `[10 x x86_amx]`
/// with `LLParser::parseArrayVectorType`'s `invalid array element type`,
/// anchored at its `TypeLoc` — the element type.
#[test]
fn upstream_x86_amx_array_element_is_rejected() {
    assert_eq!(
        rejection(include_bytes!("fixtures/upstream/Verifier/x86_amx1.ll")),
        ("invalid array element type".to_owned(), (3, 30))
    );
}

/// Ports `test/Verifier/x86_amx2.ll`: `<2 x x86_amx>` is rejected with
/// `LLParser::parseArrayVectorType`'s `invalid vector element type`, anchored
/// at its `TypeLoc` — the element type.
#[test]
fn upstream_x86_amx_vector_element_is_rejected() {
    assert_eq!(
        rejection(include_bytes!("fixtures/upstream/Verifier/x86_amx2.ll")),
        ("invalid vector element type".to_owned(), (5, 25))
    );
}

/// Ports `test/Verifier/target-ext-vector-invalid.ll`: `spirv.Image` does not
/// declare `CanBeVectorElement`, so `VectorType::isValidElementType` refuses
/// it and the parser reports `invalid vector element type`, anchored at
/// `LLParser::parseArrayVectorType`'s `TypeLoc` — the `target` keyword.
#[test]
fn upstream_target_type_without_the_vector_property_is_rejected() {
    assert_eq!(
        rejection(include_bytes!(
            "fixtures/upstream/Verifier/target-ext-vector-invalid.ll"
        )),
        ("invalid vector element type".to_owned(), (6, 20))
    );
}

/// Ports `test/Verifier/invalid-label-param.ll`: a `label` parameter fails
/// `FunctionType::isValidArgumentType` (`invalid type for function argument`).
///
/// **Blocked port — `docs/divergences.md` entry 145 names the blocker.**
/// Upstream raises the message from `LLParser::parseParameterList`, anchored
/// at its `ArgLoc`: the `label` in `@invalid_arg_type`'s `call`, `(5, 18)`.
/// llvmkit's call-argument loop has no `isValidArgumentType` guard, so it
/// accepts that call and raises the same text from its port of
/// `LLParser::parseArgumentList` at the `label` in the later
/// `declare void @foo(label)`. The message half is ported; the position
/// asserted is llvmkit's current one, so closing the entry fails this test.
#[test]
fn upstream_label_parameter_is_rejected() {
    let (message, at) = rejection(include_bytes!(
        "fixtures/upstream/Verifier/invalid-label-param.ll"
    ));
    assert_eq!(message, "invalid type for function argument");
    // Current behaviour (entry 145); upstream anchors at (5, 18).
    assert_eq!(at, (9, 19));
}

/// Ports `test/Verifier/target-ext-vector.ll`'s `RUN` half: a vector of
/// `target("llvm.test.vectorelement")` — the test type that declares
/// `CanBeVectorElement` — parses and verifies (`opt -passes=verify`).
///
/// **Blocked half, by name:** the fixture's check block uses `CHECK-LABEL`,
/// `CHECK-SAME` and `{{...}}` / `[[...]]` patterns, which
/// `tests/support/mod.rs::check_directives` does not implement; the printed
/// output is not compared.
#[test]
fn upstream_target_type_with_the_vector_property_verifies() {
    let m = Module::dynamic("type_validity");
    Parser::new(
        include_bytes!("fixtures/upstream/Verifier/target-ext-vector.ll").as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect("fixture parses");
    m.verify_borrowed().expect("fixture verifies");
}

/// The element kinds of the agreement grid, by `.ll` spelling.
const ELEMENTS: &[&str] = &[
    "i32",
    "float",
    "ptr",
    "void",
    "label",
    "metadata",
    "token",
    "x86_amx",
    "[2 x i32]",
    "{ i32 }",
    "<2 x i32>",
    "<vscale x 2 x i32>",
    "i32 ()",
    "target(\"llvm.test.vectorelement\")",
    "target(\"spirv.Image\")",
];

/// Builds the grid element spelled `text`. Every grid element is itself valid.
fn element<'m>(m: &'m Module<DynBrand>, text: &str) -> Result<Type<'m, DynBrand>, IrError> {
    let i32_ty = m.i32_type().as_type();
    Ok(match text {
        "i32" => i32_ty,
        "float" => m.f32_type().as_type(),
        "ptr" => m.ptr_type(0).as_type(),
        "void" => m.void_type().as_type(),
        "label" => m.label_type().as_type(),
        "metadata" => m.metadata_type().as_type(),
        "token" => m.token_type().as_type(),
        "x86_amx" => m.x86_amx_type(),
        "[2 x i32]" => m.array_type(i32_ty, 2).as_type(),
        "{ i32 }" => m.struct_type([i32_ty]).as_type(),
        "<2 x i32>" => m.vector_type(i32_ty, 2).as_type(),
        "<vscale x 2 x i32>" => m.scalable_vector_type(i32_ty, 2).as_type(),
        "i32 ()" => m.function_type_no_parameters(i32_ty).as_type(),
        "target(\"llvm.test.vectorelement\")" => m
            .target_ext_type(
                "llvm.test.vectorelement",
                Vec::<Type<'_, DynBrand>>::new(),
                Vec::<u32>::new(),
            )
            .as_type(),
        "target(\"spirv.Image\")" => m
            .target_ext_type(
                "spirv.Image",
                Vec::<Type<'_, DynBrand>>::new(),
                Vec::<u32>::new(),
            )
            .as_type(),
        other => panic!("`{other}` is not in ELEMENTS"),
    })
}

/// A position a component type can occupy.
#[derive(Clone, Copy, Debug)]
enum Container {
    Vector,
    ScalableVector,
    Array,
    Struct,
    FunctionReturn,
    FunctionParameter,
}

impl Container {
    const ALL: [Container; 6] = [
        Container::Vector,
        Container::ScalableVector,
        Container::Array,
        Container::Struct,
        Container::FunctionReturn,
        Container::FunctionParameter,
    ];

    fn label(self) -> &'static str {
        match self {
            Container::Vector => "vector",
            Container::ScalableVector => "scalable vector",
            Container::Array => "array",
            Container::Struct => "struct",
            Container::FunctionReturn => "function return",
            Container::FunctionParameter => "function parameter",
        }
    }

    /// The `.ll` spelling of this container around `element`.
    fn spell(self, element: &str) -> String {
        match self {
            Container::Vector => format!("<2 x {element}>"),
            Container::ScalableVector => format!("<vscale x 2 x {element}>"),
            Container::Array => format!("[2 x {element}]"),
            Container::Struct => format!("{{ {element} }}"),
            Container::FunctionReturn => format!("{element} ()"),
            Container::FunctionParameter => format!("void ({element})"),
        }
    }

    /// The builder's construction of this container around `element`.
    fn build<'m>(
        self,
        m: &'m Module<DynBrand>,
        element: Type<'m, DynBrand>,
    ) -> Result<Type<'m, DynBrand>, IrError> {
        Ok(match self {
            Container::Vector => m.vector_type(element, 2).as_type(),
            Container::ScalableVector => m.scalable_vector_type(element, 2).as_type(),
            Container::Array => m.array_type(element, 2).as_type(),
            Container::Struct => m.struct_type([element]).as_type(),
            Container::FunctionReturn => m.function_type_no_parameters(element).as_type(),
            Container::FunctionParameter => m.function_type(m.void_type(), [element]).as_type(),
        })
    }
}

/// `(container, element)` pairs the builder still accepts while the parser
/// refuses them. Each task of the type-constructor plan removes its own rows;
/// the list ends empty and is then deleted.
const KNOWN_DISAGREEMENTS: &[(&str, &str)] = &[
    // vector — removed by Task 2.
    ("vector", "void"),
    ("vector", "label"),
    ("vector", "metadata"),
    ("vector", "token"),
    ("vector", "x86_amx"),
    ("vector", "[2 x i32]"),
    ("vector", "{ i32 }"),
    ("vector", "<2 x i32>"),
    ("vector", "<vscale x 2 x i32>"),
    ("vector", "i32 ()"),
    ("vector", "target(\"spirv.Image\")"),
    // scalable vector — removed by Task 2.
    ("scalable vector", "void"),
    ("scalable vector", "label"),
    ("scalable vector", "metadata"),
    ("scalable vector", "token"),
    ("scalable vector", "x86_amx"),
    ("scalable vector", "[2 x i32]"),
    ("scalable vector", "{ i32 }"),
    ("scalable vector", "<2 x i32>"),
    ("scalable vector", "<vscale x 2 x i32>"),
    ("scalable vector", "i32 ()"),
    ("scalable vector", "target(\"spirv.Image\")"),
    // array — removed by Task 3.
    ("array", "void"),
    ("array", "label"),
    ("array", "metadata"),
    ("array", "token"),
    ("array", "x86_amx"),
    ("array", "i32 ()"),
    // struct — removed by Task 4.
    ("struct", "void"),
    ("struct", "label"),
    ("struct", "metadata"),
    ("struct", "token"),
    ("struct", "i32 ()"),
    // function return — removed by Task 5.
    ("function return", "label"),
    ("function return", "metadata"),
    ("function return", "i32 ()"),
    // function parameter — removed by Task 5.
    ("function parameter", "void"),
    ("function parameter", "label"),
    ("function parameter", "i32 ()"),
];

/// The builder accepts a derived type exactly when the parser accepts its
/// spelling, over every container × element kind of the grid.
/// llvmkit-specific law (D10, D11): no upstream counterpart — LLVM's builder
/// half is an `assert`, which no test can observe.
#[test]
fn the_builder_and_the_parser_agree_on_every_derived_type() {
    let m = Module::dynamic("agreement");
    let mut unexpected = Vec::new();
    let mut seen = Vec::new();
    for container in Container::ALL {
        for &text in ELEMENTS {
            let element = element(&m, text).expect("every grid element builds");
            let built = container.build(&m, element).is_ok();
            let spelled = container.spell(text);
            let parsed = parse_type(spelled.as_bytes(), &m).is_ok();
            if built == parsed {
                continue;
            }
            let pair = (container.label(), text);
            if built && !parsed && KNOWN_DISAGREEMENTS.contains(&pair) {
                seen.push(pair);
            } else {
                unexpected.push(format!(
                    "{}: `{spelled}` built={built} parsed={parsed}",
                    container.label()
                ));
            }
        }
    }
    let stale: Vec<_> = KNOWN_DISAGREEMENTS
        .iter()
        .filter(|pair| !seen.contains(pair))
        .collect();
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "unexpected: {unexpected:#?}\nstale: {stale:#?}"
    );
}

/// Each container spells its upstream syntax. llvmkit-specific: the law's own
/// positive control — a wrong spelling would make the parser refuse for the
/// wrong reason.
#[test]
fn each_container_spells_its_upstream_syntax() {
    let spelled: Vec<String> = Container::ALL.iter().map(|c| c.spell("i32")).collect();
    assert_eq!(
        spelled,
        [
            "<2 x i32>",
            "<vscale x 2 x i32>",
            "[2 x i32]",
            "{ i32 }",
            "i32 ()",
            "void (i32)",
        ]
    );
    let m = Module::dynamic("agreement");
    for text in &spelled {
        parse_type(text.as_bytes(), &m).unwrap_or_else(|e| panic!("`{text}`: {e}"));
    }
}
