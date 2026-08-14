//! Module-level parser integration tests.
//!
//! Mirrors the constructive subset of upstream
//! `unittests/AsmParser/AsmParserTest.cpp` and `test/Assembler/*.ll`
//! fixtures that Session 2 of the parser-first roadmap is responsible
//! for. Each `#[test]` cites the upstream anchor it ports.

use llvmkit_asmparser::ll_parser::Parser;
use llvmkit_ir::{AnyTypeEnum, Module, ModuleBrand, module_new};

fn parse_into<B: ModuleBrand>(src: &str, m: &Module<B>) {
    Parser::new(src.as_bytes(), m)
        .expect("lexer primes")
        .parse_module()
        .expect("parser succeeds");
}

/// Ports the `@g5`–`@g14` half of `test/Assembler/globalvariable-attributes.ll`
/// verbatim, asserting each of its CHECK lines.
///
/// `@g9` is the one that earns its place: it writes three sanitizer keywords
/// and pins that they print in `printGlobal`'s fixed order — address,
/// hwaddress, memtag, dyninit — and that the whole group comes *before*
/// `align`. `parseSanitizer` merges into whatever the global already carries,
/// so the keywords accumulate rather than replacing each other.
///
/// `@g1`–`@g4` are not ported: they need the trailing global attribute list
/// and the attribute-group printer, which are W7 work and recorded in
/// `docs/future-work.md`.
#[test]
fn global_sanitizer_and_code_model_round_trip() {
    for (source, expected) in [
        (
            "@g5 = global i32 2, no_sanitize_address, align 4\n",
            "@g5 = global i32 2, no_sanitize_address, align 4",
        ),
        (
            "@g6 = global i32 2, no_sanitize_hwaddress, align 4\n",
            "@g6 = global i32 2, no_sanitize_hwaddress, align 4",
        ),
        (
            "@g7 = global i32 2, sanitize_address_dyninit, align 4\n",
            "@g7 = global i32 2, sanitize_address_dyninit, align 4",
        ),
        (
            "@g8 = global i32 2, sanitize_memtag, align 4\n",
            "@g8 = global i32 2, sanitize_memtag, align 4",
        ),
        (
            "@g9 = global i32 2, no_sanitize_address, no_sanitize_hwaddress, sanitize_memtag, align 4\n",
            "@g9 = global i32 2, no_sanitize_address, no_sanitize_hwaddress, sanitize_memtag, align 4",
        ),
        (
            "@g10 = global i32 2, code_model \"tiny\"\n",
            "@g10 = global i32 2, code_model \"tiny\"",
        ),
        (
            "@g11 = global i32 2, code_model \"small\"\n",
            "@g11 = global i32 2, code_model \"small\"",
        ),
        (
            "@g12 = global i32 2, code_model \"kernel\"\n",
            "@g12 = global i32 2, code_model \"kernel\"",
        ),
        (
            "@g13 = global i32 2, code_model \"medium\"\n",
            "@g13 = global i32 2, code_model \"medium\"",
        ),
        (
            "@g14 = global i32 2, code_model \"large\"\n",
            "@g14 = global i32 2, code_model \"large\"",
        ),
    ] {
        let m = llvmkit_ir::Module::dynamic("global_properties");
        parse_into(source, &m);
        let printed = format!("{m}");
        assert!(printed.contains(expected), "{source}printed:\n{printed}");

        // Same module name, so the `ModuleID` header matches and the
        // comparison is about the global itself.
        let round_tripped = llvmkit_ir::Module::dynamic("global_properties");
        parse_into(printed.as_str(), &round_tripped);
        assert_eq!(format!("{round_tripped}"), printed);
    }
}

/// `parseOptionalCodeModel` binds its message to a local and uses it for both
/// failures, so an unrecognised model and a non-string token report the same
/// thing. Anchored on the symbol; no upstream `.ll` exercises the failure.
#[test]
fn a_bad_code_model_reports_upstream_text() {
    for source in [
        "@g = global i32 2, code_model \"huge\"\n",
        "@g = global i32 2, code_model 4\n",
    ] {
        let m = llvmkit_ir::Module::dynamic("bad_code_model");
        let err = Parser::new(source.as_bytes(), &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("bad code model is rejected")
            .to_string();
        assert_eq!(err, "expected global code model string", "{source}");
    }
}

/// Ports the whole twelve-fixture family
/// `test/Assembler/{internal,private}-{hidden,protected}-{alias,function,variable}.ll`
/// verbatim, each asserting its CHECK line.
///
/// The family exists because upstream asks the same pair —
/// `isValidVisibilityForLinkage` and `isValidDLLStorageClassForLinkage` — at
/// three separate call sites, and the fixtures cover all three. llvmkit had
/// the checks on the *alias* path only, so `@var = internal hidden global i32 0`
/// and `define internal hidden void @f()` were both accepted. The predicate is
/// now one function the three sites share, which is why porting eight of the
/// twelve and leaving four would have been the wrong split.
#[test]
fn local_linkage_constrains_visibility_everywhere() {
    fn parse_err(src: &str) -> String {
        let m = llvmkit_ir::Module::dynamic("local_linkage");
        Parser::new(src.as_bytes(), &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("local linkage with non-default visibility is rejected")
            .to_string()
    }

    for linkage in ["internal", "private"] {
        for visibility in ["hidden", "protected"] {
            // ...-variable.ll
            assert_eq!(
                parse_err(&format!("@var = {linkage} {visibility} global i32 0\n")),
                "symbol with local linkage must have default visibility",
                "{linkage} {visibility} variable"
            );
            // ...-alias.ll
            assert_eq!(
                parse_err(&format!(
                    "@global = global i32 0\n@alias = {linkage} {visibility} alias i32, ptr @global\n"
                )),
                "symbol with local linkage must have default visibility",
                "{linkage} {visibility} alias"
            );
            // ...-function.ll
            assert_eq!(
                parse_err(&format!(
                    "define {linkage} {visibility} void @function() {{\nentry:\n  ret void\n}}\n"
                )),
                "symbol with local linkage must have default visibility",
                "{linkage} {visibility} function"
            );
        }
    }
}

/// The DLL-storage-class half of the same pair, which upstream has no fixture
/// for — `isValidDLLStorageClassForLinkage` is checked immediately after the
/// visibility one at all three sites, so it is anchored on the symbol.
#[test]
fn local_linkage_constrains_dll_storage_class_everywhere() {
    fn parse_err(src: &str) -> String {
        let m = llvmkit_ir::Module::dynamic("local_linkage_dll");
        Parser::new(src.as_bytes(), &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("local linkage with a DLL storage class is rejected")
            .to_string()
    }

    for src in [
        "@var = internal dllexport global i32 0\n",
        "@global = global i32 0\n@alias = private dllexport alias i32, ptr @global\n",
        "define internal dllexport void @function() {\nentry:\n  ret void\n}\n",
    ] {
        assert_eq!(
            parse_err(src),
            "symbol with local linkage cannot have a DLL storage class",
            "{src}"
        );
    }
}

/// `invalid type for global variable`, which llvmkit had no check for.
///
/// Two halves, and the second is the easy one to miss: `Ty->isFunctionTy()`
/// rejects a global whose value type is a function, and
/// `PointerType::isValidElementType` rejects `void`, `label`, `metadata`,
/// `token` and `x86_amx` — a global's value type is the pointee of its own
/// `ptr`, so it must be a legal pointer element.
///
/// Anchored at the *type*, and checked after the initializer, both of which
/// are upstream's ordering. No upstream `.ll` pins it.
#[test]
fn a_global_of_invalid_type_is_rejected() {
    fn parse_err(src: &str) -> String {
        let m = llvmkit_ir::Module::dynamic("invalid_global_type");
        Parser::new(src.as_bytes(), &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("invalid global value type is rejected")
            .to_string()
    }

    for src in [
        // The function-type case needs a declaration linkage to reach the
        // check: with an initializer to parse, `parseGlobalValue` fails first
        // — upstream included, since the type check runs after it.
        "@g = external global void ()\n",
        "@g = external global label\n",
        "@g = external global metadata\n",
        "@g = external global token\n",
    ] {
        assert_eq!(parse_err(src), "invalid type for global variable", "{src}");
    }

    // Bare `void` never reaches this check: `parseType` is called with
    // `AllowVoid = false` and refuses it first, with its own message.
    assert_eq!(
        parse_err("@g = external global void\n"),
        "void type only allowed for function results"
    );
}

/// A declaration linkage means upstream never *looks* for an initializer —
/// `if (!HasLinkage || !isValidDeclarationLinkage(Linkage))` guards the
/// `parseGlobalValue` call, and there is no lookahead behind it.
///
/// So `@g = external global i32 0` leaves the `0` unconsumed and it fails at
/// top level. llvmkit used to peek ahead and report an invented
/// `no initializer: a global with 'external' linkage is a declaration` —
/// the same rejection reached by guessing rather than by the rule.
#[test]
fn a_declaration_linkage_global_takes_no_initializer() {
    let m = llvmkit_ir::Module::dynamic("declaration_linkage");
    let err = Parser::new(b"@g = external global i32 0\n".as_slice(), &m)
        .expect("lexer primes")
        .parse_module()
        .expect_err("the initializer is never consumed")
        .to_string();
    assert_eq!(err, "expected top-level entity");

    // Without the initializer it is an ordinary declaration.
    let m = llvmkit_ir::Module::dynamic("declaration_linkage_ok");
    parse_into("@g = external global i32\n", &m);
    assert!(format!("{m}").contains("@g = external global i32"), "{m}");
}

/// An `ifunc` with a linkage `GlobalIFunc::isValidLinkage` rejects **parses**,
/// and is caught by the verifier.
///
/// `parseAliasOrIFunc` guards its `isValidLinkage` call with
/// `if (IsAlias && ...)`, so upstream's parser checks aliases only and
/// `Verifier::visitGlobalIFunc` carries the ifunc rule. llvmkit rejected it at
/// parse time *and* in `GlobalIfuncBuilder::build`, which is stricter than
/// upstream — a divergence in its own right — and made the real diagnostic
/// unreachable. The alias half stays a parse error, as upstream has it.
#[test]
fn an_ifunc_linkage_is_a_verifier_rule_not_a_parse_rule() {
    let m = llvmkit_ir::Module::dynamic("ifunc_linkage");
    Parser::new(
        b"declare ptr @r()\n@i = appending ifunc i32 (i32), ptr @r\n".as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect("upstream's parser accepts any ifunc linkage");

    let err = m.verify().expect_err("the verifier rejects it").to_string();
    assert!(
        err.contains(
            "IFunc should have private, internal, linkonce, weak, linkonce_odr, \
             weak_odr, or external linkage!"
        ),
        "got: {err}"
    );

    // The alias twin is a *parse* error, because upstream checks that one.
    let m = llvmkit_ir::Module::dynamic("alias_linkage");
    let err = Parser::new(
        b"@g = global i32 0\n@a = appending alias i32, ptr @g\n".as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect_err("an alias linkage is checked at parse time")
    .to_string();
    assert!(err.contains("invalid linkage type for alias"), "got: {err}");
}

/// The module-entity diagnostics that name a *property*, each verbatim.
///
/// Three of them are prose that does not begin with "expected", and llvmkit
/// had routed two through the `Expected` variant — which rendered
/// `expected unknown alias or ifunc property`, gluing the word onto a message
/// that is not one. Note the bangs: upstream ends both property messages with
/// `!` and neither is a typo to tidy.
///
/// `Metadata id is already used` capitalises its first word, joining
/// `parseScope`'s and `parseOrdering`'s messages as the only ones that do.
///
/// Each trigger is a real keyword in the wrong place (`nounwind`), not a
/// misspelling: llvmkit's lexer answers `unknown keyword '...'` for a word it
/// does not know, so a misspelled trigger never reaches the parser. That is
/// the same re-layering the `memory(...)` and `uwtable` fixtures wait on.
#[test]
fn module_entity_property_diagnostics_match_upstream_text() {
    fn parse_err(src: &str) -> String {
        let m = llvmkit_ir::Module::dynamic("module_entity_diagnostics");
        Parser::new(src.as_bytes(), &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("malformed module entity is rejected")
            .to_string()
    }

    assert_eq!(
        parse_err("target nounwind = \"x\"\n"),
        "unknown target property"
    );
    assert_eq!(
        parse_err("@g = global i32 0, nounwind\n"),
        "unknown global variable property!"
    );
    assert_eq!(
        parse_err("@t = global i32 0\n@a = alias i32, ptr @t, nounwind\n"),
        "unknown alias or ifunc property!"
    );
    assert_eq!(
        parse_err("@t = global i32 0\n@a = alias i32, i32 7\n"),
        "An alias or ifunc must have pointer type"
    );
    assert_eq!(
        parse_err("!0 = !{}\n!0 = !{}\n"),
        "Metadata id is already used"
    );
    // "Detect common error, from old metadata syntax": `!0 = metadata !{}`
    // was once legal, so a type token here gets its own message.
    assert_eq!(
        parse_err("!0 = i32 !{}\n"),
        "unexpected type in metadata definition"
    );
}

/// Mirrors `test/Assembler/datalayout.ll` + the trailing `target triple`
/// arm: a module that carries both directives round-trips through the
/// AsmWriter byte-for-byte.
#[test]
fn target_directives_round_trip_through_asm_writer() {
    let printed = {
        let m = module_new!("target_directives_round_trip").expect("fresh module");
        parse_into(
            "target datalayout = \"e-m:e-i64:64\"\ntarget triple = \"x86_64-unknown-linux-gnu\"\n",
            &m,
        );
        format!("{m}")
    };
    assert!(printed.contains("target datalayout = \"e-m:e-i64:64\""));
    assert!(printed.contains("target triple = \"x86_64-unknown-linux-gnu\""));
}
/// Mirrors `LLParser::parseSourceFileName` and `AsmWriter.cpp`'s
/// `getSourceFileName()` print arm: the directive is stored on the
/// module and re-emitted immediately after the `ModuleID` comment.
#[test]
fn source_filename_round_trips_through_asm_writer() {
    let printed = {
        let m = module_new!("source_file").expect("fresh module");
        parse_into("source_filename = \"dir/file.c\"\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("; ModuleID = 'source_file'\nsource_filename = \"dir/file.c\"\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors `LLParser::parseComdat`: a top-level `$name = comdat <kind>`
/// directive creates the module COMDAT entry and AsmWriter re-emits it.
#[test]
fn top_level_comdat_round_trips() {
    let printed = {
        let m = module_new!("comdat_module").expect("fresh module");
        parse_into("$foo = comdat largest\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("$foo = comdat largest\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the `externally_initialized` flag in `LLParser::parseGlobal`.
#[test]
fn global_externally_initialized_round_trips() {
    let printed = {
        let m = module_new!("global_externally_initialized").expect("fresh module");
        parse_into("@g = externally_initialized global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = externally_initialized global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the full linkage-prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_linkage_round_trips() {
    let printed = {
        let m = module_new!("global_linkage").expect("fresh module");
        parse_into("@g = weak_odr global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = weak_odr global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the visibility-prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_visibility_round_trips() {
    let printed = {
        let m = module_new!("global_visibility").expect("fresh module");
        parse_into("@g = hidden global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = hidden global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the DLL storage class prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_dll_storage_round_trips() {
    let printed = {
        let m = module_new!("global_dll_storage").expect("fresh module");
        parse_into("@g = dllexport global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = dllexport global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}
/// Mirrors the thread-local mode prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_tls_mode_round_trips() {
    let printed = {
        let m = module_new!("global_tls").expect("fresh module");
        parse_into("@g = thread_local(initialexec) global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = thread_local(initialexec) global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the unnamed-address prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_unnamed_addr_round_trips() {
    let printed = {
        let m = module_new!("global_unnamed_addr").expect("fresh module");
        parse_into("@g = local_unnamed_addr global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = local_unnamed_addr global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the address-space prefix arm of `LLParser::parseGlobal`.
#[test]
fn global_addrspace_round_trips() {
    let printed = {
        let m = module_new!("global_addrspace").expect("fresh module");
        parse_into("@g = addrspace(3) global i32 0\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("@g = addrspace(3) global i32 0\n"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors the global-object suffix loop in `LLParser::parseGlobal` for
/// section, partition, explicit COMDAT attachment, and alignment.
#[test]
fn global_trailing_attributes_round_trip() {
    let printed = {
        let m = module_new!("global_trailing_attrs").expect("fresh module");
        parse_into(
            "$foo = comdat any\n@g = global i32 0, section \".data\", partition \"part\", comdat($foo), align 8\n",
            &m,
        );
        format!("{m}")
    };
    assert!(
        printed.contains(
            "@g = global i32 0, section \".data\", partition \"part\", comdat($foo), align 8\n"
        ),
        "AsmWriter output: {printed}"
    );
}

/// Ports the `module asm` arm of `test/Assembler/module-asm.ll`. Multiple
/// directives accumulate, separated by newlines as upstream's
/// `printModuleInlineAsm` emits.
#[test]
fn module_asm_directives_accumulate() {
    let asm = {
        let m = module_new!("module_asm").expect("fresh module");
        parse_into(
            "module asm \"first line\"\nmodule asm \"second line\"\n",
            &m,
        );
        m.module_asm()
    };
    assert!(asm.contains("first line"));
    assert!(asm.contains("second line"));
}

/// Mirrors `test/Assembler/named-types.ll`: a recursive named-struct
/// forward reference resolves once the matching `%foo = type { ... }`
/// definition is encountered.
#[test]
fn named_struct_forward_reference_resolves() {
    let m = module_new!("recursive_named").expect("fresh module");
    let parser = Parser::new(b"%self = type { ptr }\n", &m).unwrap();
    let parsed = parser.parse_module().expect("parser succeeds");
    // The named-type table records the definition so external callers can
    // resolve `%self` against this module.
    assert!(parsed.slot_mapping.named_types.contains_key("self"));
}

/// Ports the `[N x T]` and `<N x T>` arms of `LLParser::parseType`. The
/// declarator ingestion proves both compositions work end-to-end.
#[test]
fn array_and_vector_types_parse() {
    let m = module_new!("aggregate_types").expect("fresh module");
    parse_into("declare void @takes([4 x i32], <8 x float>)\n", &m);
    let f = m.view(m.function_dyn("takes").expect("function present"));
    let params: Vec<_> = f.signature().params().collect();
    assert_eq!(params.len(), 2);
    assert!(matches!(
        AnyTypeEnum::from(params[0]),
        AnyTypeEnum::Array(_)
    ));
    let v = match AnyTypeEnum::from(params[1]) {
        AnyTypeEnum::Vector(v) => v,
        other => panic!("expected vector type, got {other:?}"),
    };
    assert!(!v.is_scalable());
}

/// Ports the `<vscale x N x T>` arm of `LLParser::parseType`.
#[test]
fn scalable_vector_type_parses() {
    let m = module_new!("scalable_vec").expect("fresh module");
    parse_into("declare void @sv(<vscale x 4 x i32>)\n", &m);
    let f = m.view(m.function_dyn("sv").expect("function present"));
    let params: Vec<_> = f.signature().params().collect();
    let v = match AnyTypeEnum::from(params[0]) {
        AnyTypeEnum::Vector(v) => v,
        other => panic!("expected vector type, got {other:?}"),
    };
    assert!(v.is_scalable());
}

/// Mirrors `test/Assembler/declare.ll`: a varargs declaration whose
/// signature round-trips through the AsmWriter.
#[test]
fn variadic_declaration_round_trips() {
    let printed = {
        let m = module_new!("variadic_decl").expect("fresh module");
        parse_into("declare i32 @printf(ptr, ...)\n", &m);
        format!("{m}")
    };
    assert!(
        printed.contains("declare i32 @printf(ptr, ...)")
            || printed.contains("declare i32 @printf(ptr %0, ...)"),
        "AsmWriter output: {printed}"
    );
}

/// Mirrors `test/Assembler/global-variable-attributes.ll` (the integer
/// arm). The numbered-global slot table tracks `@0`, `@1`, etc. like
/// upstream's `NumberedVals`.
#[test]
fn numbered_global_records_in_slot_mapping() {
    let m = module_new!("numbered_globals").expect("fresh module");
    let parser = Parser::new(b"@0 = global i32 0\n@1 = global i32 1\n", &m).unwrap();
    let parsed = parser.parse_module().expect("parser succeeds");
    assert_eq!(parsed.slot_mapping.global_values.next_unused_id(), 2);
    assert!(parsed.slot_mapping.global_values.get(0).is_some());
    assert!(parsed.slot_mapping.global_values.get(1).is_some());
}

/// Ports `LLParser::parseType`'s `AllowVoid` guard: void is rejected outside
/// function-result position, with upstream's text.
///
/// This used to assert llvmkit's own wording — `expected non-void type (void
/// only allowed at function results)` — under a doc comment that named
/// upstream's and called the difference "a structured error". It was neither
/// structured nor upstream's; it was a divergence documented instead of
/// fixed.
#[test]
fn void_in_value_position_is_rejected() {
    let err = {
        let m = module_new!("reject_void").expect("fresh module");
        let parser = Parser::new(b"@x = global void 0\n", &m).unwrap();
        parser.parse_module().unwrap_err()
    };
    assert_eq!(
        err.to_string(),
        "void type only allowed for function results"
    );
}

/// Mirrors `LLParser::parseTopLevelEntities`'s default arm: any unknown
/// leading token is reported as a typed `top-level entity` error.
#[test]
fn unknown_top_level_entity_is_typed_error() {
    let err = {
        let m = module_new!("unknown_top_level").expect("fresh module");
        let parser = Parser::new(b"42 i32\n", &m).unwrap();
        parser.parse_module().unwrap_err()
    };
    assert!(format!("{err}").contains("top-level entity"));
}
