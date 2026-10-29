//! Which derived types exist: the `test/Verifier` fixtures that pin the
//! parser's half of each component rule, and a law tying the builder's half
//! to it.
//!
//! LLVM spells every component rule twice — an `assert` in the type's
//! constructor (`FixedVectorType::get`, `ArrayType::get`,
//! `FunctionType::FunctionType`) or no check at all (`StructType::get`), and an
//! error in `LLParser` — and an `assert` is compiled out of a release build.
//!
//! Over the agreement grid below (6 containers × 25 element kinds, 150 cells),
//! llvmkit's builder and parser agree on every cell: none is built but not
//! parsed, none is parsed but not built, and where both accept they make the
//! same type. Measured at the commit that makes the function-type constructors
//! refuse an invalid result or parameter (parent `ad8261a`) by running
//! `cargo +1.96.0 test --release -p llvmkit-asmparser --test type_validity`
//! with `KNOWN_DISAGREEMENTS` empty. Built but not parsed, it was 46 at
//! `5ce065f`, before the vector constructors refused their 28; 18 at the
//! commit that adds `llvmkit_ir::VectorType::of` (parent `dcde8b6`), before the
//! array constructor refused its 6; 12 at the commit that adds
//! `llvmkit_ir::ArrayType::of` (parent `bb7e518`), before the struct
//! constructors refused their 5; and 7 at the commit that makes them refuse
//! (parent `9b4bce3`), before the function-type constructors refused their 7.
//! Each of those cells was a type a module built in Rust could hold and print,
//! and no parser read back.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_asmparser::{SlotMapping, parse_type, parse_type_at_beginning, parse_type_with_slots};
use llvmkit_ir::{AnyTypeEnum, ArrayType, DynBrand, IrError, Module, Type, VectorType, lanes};

pub mod support;

use support::rejection_at;

/// Ports `test/Verifier/x86_amx1.ll`: `llvm-as` rejects `[10 x x86_amx]`
/// with `LLParser::parseArrayVectorType`'s `invalid array element type`,
/// anchored at its `TypeLoc` — the element type. The fixture's `CHECK` pins
/// only the text; the position is the routine's anchor.
#[test]
fn upstream_x86_amx_array_element_is_rejected() {
    assert_eq!(
        rejection_at(include_bytes!("fixtures/upstream/Verifier/x86_amx1.ll")),
        ("invalid array element type".to_owned(), (3, 30))
    );
}

/// Ports `test/Verifier/x86_amx2.ll`: `<2 x x86_amx>` is rejected with
/// `LLParser::parseArrayVectorType`'s `invalid vector element type`, anchored
/// at its `TypeLoc` — the element type.
#[test]
fn upstream_x86_amx_vector_element_is_rejected() {
    assert_eq!(
        rejection_at(include_bytes!("fixtures/upstream/Verifier/x86_amx2.ll")),
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
        rejection_at(include_bytes!(
            "fixtures/upstream/Verifier/target-ext-vector-invalid.ll"
        )),
        ("invalid vector element type".to_owned(), (6, 20))
    );
}

/// Ports `test/Verifier/invalid-label-param.ll`: a `label` parameter fails
/// `FunctionType::isValidArgumentType` (`invalid type for function argument`).
///
/// The fixture's `CHECK` pins only the text. Upstream raises it from
/// `LLParser::parseParameterList`, as `error(ArgLoc, …)`, at the `label` in
/// `@invalid_arg_type`'s `call` — before the later `declare void @foo(label)`
/// is read, whose own refusal comes from `LLParser::parseArgumentList` with
/// the same text. Comparing the text alone cannot tell the two sites apart,
/// so the position is asserted too.
#[test]
fn upstream_label_parameter_is_rejected() {
    assert_eq!(
        rejection_at(include_bytes!(
            "fixtures/upstream/Verifier/invalid-label-param.ll"
        )),
        ("invalid type for function argument".to_owned(), (5, 18))
    );
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

/// An array constant whose elements are first-class yet no array element —
/// `x86_amx undef`, `token none` — is refused at its first element, with the
/// `lsquare` arm's own `invalid array element type` diagnostic, before the
/// element-agreement loop runs: a later element of another type does not
/// change the answer.
///
/// llvmkit-specific regression (Doctrine D10; `docs/divergences.md` entry D18):
/// `LLParser::parseValID`'s `lsquare` arm checks only `isFirstClassType`
/// before calling `ArrayType::get`, whose `isValidElementType` assert is the
/// only check these elements meet, and runs its agreement loop after it; a
/// release build makes `[2 x x86_amx]` and then reports
/// `convertValIDToValue`'s `constant expression type mismatch`, which is what
/// llvmkit reported too until its array constructor refused the element. No
/// upstream test reaches the assert: against the vendored `llvmorg-22.1.4`
/// tree, `rg -n '\[\s*(x86_amx|token) ' llvm/test llvm/unittests` finds only
/// `test/MC/M68k/operand.s`, an assembler `CHECK` line. A well-typed array
/// constant is the positive control.
#[test]
fn an_array_constant_of_a_non_array_element_is_refused_at_its_first_element() {
    for (source, message) in [
        (
            "@g = global i32 [x86_amx undef, x86_amx undef]\n",
            "invalid array element type: x86_amx",
        ),
        (
            "@g = global i32 [token none, token none]\n",
            "invalid array element type: token",
        ),
        (
            "@g = global i32 [x86_amx undef, i32 1]\n",
            "invalid array element type: x86_amx",
        ),
    ] {
        assert_eq!(
            rejection_at(source),
            (message.to_owned(), (1, 18)),
            "{source}"
        );
    }
    let m = Module::dynamic("type_validity");
    Parser::new(b"@g = global [2 x i32] [i32 1, i32 2]\n".as_slice(), &m)
        .expect("lexer primes")
        .parse_module()
        .expect("a well-typed array constant parses");
}

/// `llvm::parseType` and `llvm::parseTypeAtBeginning` refuse a bare `void`:
/// `LLParser::parseTypeAtBeginning` calls `parseType(Ty)`, and `LLParser.h`
/// declares `parseType`'s `AllowVoid` parameter with a `false` default, so the
/// suffix loop's `default:` arm reports
/// `error(TypeLoc, "void type only allowed for function results")` — anchored
/// at the `void` token itself, which is what the span assertion checks.
///
/// llvmkit-specific (rule anchor): no upstream test parses a bare `void`
/// standalone (`rg -n 'parseType(AtBeginning)?\("void' llvm/unittests` finds
/// nothing in the vendored `llvmorg-22.1.4` tree); the refusal is derived from
/// the routine.
#[test]
fn a_bare_void_is_refused_by_the_standalone_type_parser() {
    const SOURCE: &[u8] = b"void";
    let m = Module::dynamic("type_validity");
    for error in [
        parse_type(SOURCE, &m).expect_err("`parse_type` refuses a bare `void`"),
        parse_type_at_beginning(SOURCE, &m)
            .expect_err("`parse_type_at_beginning` refuses a bare `void`"),
    ] {
        assert_eq!(
            error.to_string(),
            "void type only allowed for function results"
        );
        let span = error.loc();
        let start = usize::try_from(span.start).expect("span start fits in usize");
        let end = usize::try_from(span.end).expect("span end fits in usize");
        assert_eq!(&SOURCE[start..end], b"void");
        assert_eq!(support::reported_line_and_column(SOURCE, &error), (1, 1));
    }
}

/// The module every grid test parses first: it declares the grid's one named
/// type. Mirrors `unittests/AsmParser/AsmParserTest.cpp`'s
/// `TEST(AsmParserTest, TypeWithSlotMappingParsing)`, which parses a module
/// with `parseAssemblyString(Source, Error, Ctx, &Mapping)` and then resolves
/// `%st` in a standalone type through `parseType(…, &Mapping)`.
const GRID_SOURCE: &[u8] = b"%opaque = type opaque\n";

/// Parses [`GRID_SOURCE`] into `m` and returns the `SlotMapping` every grid
/// spelling is parsed through.
fn grid_slots(m: &Module<DynBrand>) -> SlotMapping<'_, DynBrand> {
    Parser::new(GRID_SOURCE, m)
        .expect("lexer primes")
        .parse_module()
        .expect("the grid's module parses")
        .slot_mapping
}

/// The element kinds of the agreement grid, by `.ll` spelling.
const ELEMENTS: &[&str] = &[
    "i1",
    "i32",
    "half",
    "bfloat",
    "float",
    "double",
    "x86_fp80",
    "fp128",
    "ppc_fp128",
    "ptr",
    "void",
    "label",
    "metadata",
    "token",
    "x86_amx",
    "[2 x i32]",
    "{ i32 }",
    "<{ i32 }>",
    "%opaque",
    "<2 x i32>",
    "<vscale x 2 x i32>",
    "i32 ()",
    "target(\"llvm.test.vectorelement\")",
    "target(\"spirv.Image\")",
    "target(\"aarch64.svcount\")",
];

/// Builds the grid element spelled `text`. Every grid element is itself valid.
/// `m` must already hold [`GRID_SOURCE`], where `%opaque` is looked up.
fn element<'m>(m: &'m Module<DynBrand>, text: &str) -> Result<Type<'m, DynBrand>, IrError> {
    let i32_ty = m.i32_type().as_type();
    let target = |name: &str| -> Result<Type<'m, DynBrand>, IrError> {
        Ok(
            m.target_ext_type(name, Vec::<Type<'_, DynBrand>>::new(), Vec::<u32>::new())?
                .as_type(),
        )
    };
    Ok(match text {
        "i1" => m.i1_type().as_type(),
        "i32" => i32_ty,
        "half" => m.half_type().as_type(),
        "bfloat" => m.bfloat_type().as_type(),
        "float" => m.f32_type().as_type(),
        "double" => m.f64_type().as_type(),
        "x86_fp80" => m.x86_fp80_type().as_type(),
        "fp128" => m.fp128_type().as_type(),
        "ppc_fp128" => m.ppc_fp128_type().as_type(),
        "ptr" => m.ptr_type(0).as_type(),
        "void" => m.void_type().as_type(),
        "label" => m.label_type().as_type(),
        "metadata" => m.metadata_type().as_type(),
        "token" => m.token_type().as_type(),
        "x86_amx" => m.x86_amx_type(),
        "[2 x i32]" => ArrayType::of(m.i32_type(), 2).as_type(),
        "{ i32 }" => m.struct_type([i32_ty])?.as_type(),
        "<{ i32 }>" => m.packed_struct_type([i32_ty])?.as_type(),
        "%opaque" => m
            .named_struct("opaque")
            .expect("GRID_SOURCE declares `%opaque`")
            .as_type(),
        "<2 x i32>" => VectorType::of(m.i32_type(), lanes::<2>()).as_type(),
        "<vscale x 2 x i32>" => VectorType::scalable_of(m.i32_type(), lanes::<2>()).as_type(),
        "i32 ()" => m.function_type_no_parameters(i32_ty)?.as_type(),
        "target(\"llvm.test.vectorelement\")" => target("llvm.test.vectorelement")?,
        "target(\"spirv.Image\")" => target("spirv.Image")?,
        "target(\"aarch64.svcount\")" => target("aarch64.svcount")?,
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
            Container::Vector => m.vector_type(element, lanes::<2>())?.as_type(),
            Container::ScalableVector => m.scalable_vector_type(element, lanes::<2>())?.as_type(),
            Container::Array => m.array_type(element, 2)?.as_type(),
            Container::Struct => m.struct_type([element])?.as_type(),
            Container::FunctionReturn => m.function_type_no_parameters(element)?.as_type(),
            Container::FunctionParameter => m.function_type(m.void_type(), [element])?.as_type(),
        })
    }
}

/// `(container, element)` pairs the builder still accepts while the parser
/// refuses them. Each task of the type-constructor plan removes its own rows;
/// the list ends empty and is then deleted. A task that removes rows here
/// re-measures the module doc's "built but not parsed" count in that commit.
const KNOWN_DISAGREEMENTS: &[(&str, &str)] = &[];

/// The builder accepts a derived type exactly when the parser accepts its
/// spelling, over every container × element kind of the grid — and where both
/// accept, they make the same type.
///
/// llvmkit-specific law (D10, D11), no upstream counterpart: LLVM's builder
/// half is an `assert`, and no upstream unit test exercises one of these
/// asserts —
/// `rg -n -A 3 "EXPECT_DEATH|ASSERT_DEATH" llvm/unittests | rg -i "VectorType::get|ArrayType::get|StructType::(get|create)|FunctionType::get|PointerType::get|TypedPointerType::get|isValidElementType|isValidReturnType|isValidArgumentType"`
/// prints nothing against the vendored `llvmorg-22.1.4` tree.
#[test]
fn the_builder_and_the_parser_agree_on_every_derived_type() {
    let m = Module::dynamic("agreement");
    let slots = grid_slots(&m);
    let mut unexpected = Vec::new();
    let mut seen = Vec::new();
    for container in Container::ALL {
        for &text in ELEMENTS {
            let element = element(&m, text).expect("every grid element builds");
            let spelled = container.spell(text);
            let built = container.build(&m, element);
            let parsed = parse_type_with_slots(spelled.as_bytes(), &m, &slots);
            let pair = (container.label(), text);
            match (built, parsed) {
                (Ok(built), Ok(parsed)) => {
                    if built != parsed {
                        unexpected.push(format!(
                            "{}: `{spelled}` built `{built}` but parsed `{parsed}`",
                            container.label()
                        ));
                    }
                }
                (Err(_), Err(_)) => {}
                (Ok(_), Err(_)) if KNOWN_DISAGREEMENTS.contains(&pair) => seen.push(pair),
                (built, parsed) => unexpected.push(format!(
                    "{}: `{spelled}` built={} parsed={}",
                    container.label(),
                    built.is_ok(),
                    parsed.is_ok()
                )),
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

/// Each element's spelling parses to the very type [`element`] builds — the
/// same interned [`Type`] in the one module. llvmkit-specific: the law's
/// positive control for its elements, as
/// [`each_container_spells_its_upstream_syntax`] is for its containers. A
/// misspelled element would otherwise pass as agreement in every container
/// where both halves refuse.
///
/// `void` is read back as the result of `void ()`, because `llvm::parseType`
/// refuses a bare `void`: `LLParser::parseTypeAtBeginning` calls `parseType`
/// with `AllowVoid` left at its `false` default
/// ([`a_bare_void_is_refused_by_the_standalone_type_parser`]).
#[test]
fn each_element_spells_the_type_the_builder_builds() {
    let m = Module::dynamic("agreement");
    let slots = grid_slots(&m);
    for &text in ELEMENTS {
        let built = element(&m, text).expect("every grid element builds");
        let parsed = if text == "void" {
            let function = parse_type_with_slots(b"void ()", &m, &slots).expect("`void ()` parses");
            match AnyTypeEnum::from(function) {
                AnyTypeEnum::Function(function) => function.return_type(),
                _ => panic!("`void ()` parses to a function type"),
            }
        } else {
            parse_type_with_slots(text.as_bytes(), &m, &slots)
                .unwrap_or_else(|e| panic!("`{text}`: {e}"))
        };
        assert_eq!(
            parsed, built,
            "`{text}`: parsed `{parsed}`, built `{built}`"
        );
    }
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
