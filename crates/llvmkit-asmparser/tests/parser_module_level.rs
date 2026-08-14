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

/// `parseGlobal`'s redefinition route: a name already in the module is
/// `redefinition of global '@x'`, unless it is there as a *forward reference*,
/// which this definition satisfies instead.
#[test]
fn a_redefined_global_is_rejected_but_a_forward_reference_is_not() {
    let m = llvmkit_ir::Module::dynamic("global_redefinition");
    let err = Parser::new(b"@g = global i32 0\n@g = global i32 1\n".as_slice(), &m)
        .expect("lexer primes")
        .parse_module()
        .expect_err("a second definition of @g is a redefinition")
        .to_string();
    assert_eq!(err, "redefinition of global '@g'");

    // The forward-reference half: `@p` names `@g` before `@g` exists, and the
    // later definition satisfies the reference rather than colliding with it.
    let m = llvmkit_ir::Module::dynamic("global_forward");
    parse_into("@p = global ptr @g\n@g = global i32 0\n", &m);
    assert!(format!("{m}").contains("@p = global ptr @g"), "{m}");
}

/// An `ifunc` accepts metadata attachments and an alias does not.
///
/// `parseAliasOrIFunc`'s property loop guards that arm with
/// `!IsAlias && Lex.getKind() == lltok::MetadataVar`, so the alias spelling
/// falls through to `unknown alias or ifunc property!`. llvmkit accepted
/// neither.
#[test]
fn an_ifunc_takes_metadata_attachments_but_an_alias_does_not() {
    let m = llvmkit_ir::Module::dynamic("ifunc_metadata");
    parse_into(
        "declare ptr @r()\n@i = ifunc i32 (i32), ptr @r, !dbg !0\n!0 = !{}\n",
        &m,
    );

    let m = llvmkit_ir::Module::dynamic("alias_metadata");
    let err = Parser::new(
        b"@g = global i32 0\n@a = alias i32, ptr @g, !dbg !0\n!0 = !{}\n".as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect_err("an alias takes no metadata attachment")
    .to_string();
    assert_eq!(err, "unknown alias or ifunc property!");
}

/// The three clauses `parseAliasOrIFunc` reads before it knows whether it has
/// an alias or an ifunc are stored on an ifunc too — upstream applies
/// `setThreadLocalMode`, `setDLLStorageClass` and `setUnnamedAddr` to `GV` in
/// both branches. It just never *prints* them for an ifunc, because
/// `printIFunc` stops after visibility, so this asserts the round-trip drops
/// them rather than asserting they reappear.
#[test]
fn an_ifunc_stores_but_does_not_print_the_shared_prefix_clauses() {
    let m = llvmkit_ir::Module::dynamic("ifunc_prefix");
    parse_into(
        "declare ptr @r()\n@i = dllexport thread_local unnamed_addr ifunc i32 (i32), ptr @r\n",
        &m,
    );
    let printed = format!("{m}");
    // `printIFunc` emits linkage, DSO location and visibility, then `ifunc` —
    // so all three clauses are dropped on the way out. That is upstream's
    // behaviour, not a llvmkit shortcut.
    assert!(
        printed.contains("@i = ifunc i32 (i32), ptr @r"),
        "{printed}"
    );
    assert!(!printed.contains("dllexport"), "{printed}");
    assert!(!printed.contains("thread_local"), "{printed}");
    assert!(!printed.contains("unnamed_addr"), "{printed}");

    let round_tripped = llvmkit_ir::Module::dynamic("ifunc_prefix");
    parse_into(printed.as_str(), &round_tripped);
    assert_eq!(format!("{round_tripped}"), printed);
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

// ── parseFunctionHeader / parseArgumentList ───────────────────────────────

fn header_err(src: &str) -> String {
    let m = llvmkit_ir::Module::dynamic("function_header");
    Parser::new(src.as_bytes(), &m)
        .expect("lexer primes")
        .parse_module()
        .expect_err("malformed function header is rejected")
        .to_string()
}

/// `parseFunctionHeader` runs its checks in one fixed order, and the order is
/// observable: the return **type** parses inside the same `||` chain as the
/// linkage, so a type that fails to parse is reported before the linkage
/// switch that would otherwise reject the header.
///
/// llvmkit used to fold the switch into `parse_optional_function_linkage`,
/// which ran it before the type was ever read.
///
/// The trigger is `=` rather than a misspelled type name: llvmkit's lexer
/// answers `unknown keyword 'x'` where upstream returns a silent error token
/// for the parser to report on, and re-layering that is W14.
#[test]
fn the_return_type_is_read_before_the_linkage_switch() {
    // `=` is not a type, so `parseType` fails first...
    assert_eq!(header_err("declare private = void @f()\n"), "expected type");
    // ...and with a well-formed type the linkage switch is what rejects it.
    assert_eq!(
        header_err("declare private void @f()\n"),
        "invalid linkage for function declaration"
    );
    assert_eq!(
        header_err("define extern_weak void @f() {\nentry:\n  ret void\n}\n"),
        "invalid linkage for function definition"
    );
    assert_eq!(
        header_err("declare common void @f()\n"),
        "invalid function linkage type"
    );
}

/// `FunctionType::isValidReturnType` (`lib/IR/Type.cpp`) rejects a function,
/// label or metadata return, checked at `RetTypeLoc` in
/// `LLParser::parseFunctionHeader`. llvmkit carried the check on the function
/// *type* production only, so the header path had none.
#[test]
fn the_header_checks_its_return_type() {
    for src in [
        "declare label @f()\n",
        "define metadata @f() {\nentry:\n  ret void\n}\n",
    ] {
        assert_eq!(header_err(src), "invalid function return type", "{src}");
    }
}

/// Ports `test/Assembler/invalid-label.ll` and
/// `test/Assembler/2007-01-02-Undefined-Arg-Type.ll`, both of which pin
/// `parseArgumentList`'s `FunctionType::isValidArgumentType` check —
/// `isFirstClassType() && !isLabelTy()`. Upstream shares `parseArgumentList`
/// between a function *type* and a function *header*, so all three paths
/// carry it; llvmkit had it on the type path only.
///
/// The second fixture is the one worth reading twice: its argument type is a
/// `%typedef.bc_struct` whose `type opaque` definition is **commented out**,
/// so the reference mints an opaque identified struct — and
/// `Type::isFirstClassType` answers false for one (`lib/IR/Type.cpp`'s
/// `StructTyID` arm asks `isOpaque`). That is why an undefined argument type
/// is caught here rather than at `validateEndOfModule`.
///
/// `test/Assembler/invalid-label-call-arg.ll` shares the message but reaches
/// it through a call's parameter list, which is W9b.
#[test]
fn the_header_checks_its_argument_types() {
    for fixture in [
        include_bytes!("fixtures/upstream/invalid-label.ll").as_slice(),
        include_bytes!("fixtures/upstream/2007-01-02-Undefined-Arg-Type.ll").as_slice(),
    ] {
        let m = llvmkit_ir::Module::dynamic("invalid_argument_type");
        let err = Parser::new(fixture, &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("fixture is rejected")
            .to_string();
        assert_eq!(err, "invalid type for function argument");
    }

    // The declaration path and the function-type production reach the same
    // check.
    for src in ["declare void @f(label)\n", "%s = type i32 (%t)\n"] {
        assert_eq!(
            header_err(src),
            "invalid type for function argument",
            "{src}"
        );
    }
}

/// `parseArgumentList`'s `checkValueID`, which llvmkit ran for a `define` and
/// **discarded** for a `declare` — the ids were parsed and thrown away.
/// Reported at the argument's *type*, which is upstream's `TypeLoc`.
///
/// The `define` half is `test/Assembler/skip-value-numbers-invalid.ll`'s
/// `arg_smaller_id` split, ported in
/// `parser_forward_refs.rs::numbered_slots_may_not_go_backwards`; this pins
/// that a *declaration* answers the same, and that a named argument consumes
/// no number — `CurValID` advances only on the unnamed branch.
#[test]
fn a_declaration_checks_its_argument_numbering() {
    assert_eq!(
        header_err("declare void @f(i32 %1, i32 %0)\n"),
        "argument expected to be numbered '%2' or greater"
    );

    // A name in between does not advance the counter, so `%1` is still the
    // next legal number after `%0`.
    let m = llvmkit_ir::Module::dynamic("named_argument_numbering");
    parse_into("declare void @f(i32 %0, i32 %named, i32 %1)\n", &m);
}

/// A *numbered* argument is legal in a function type. `parseArgumentList`
/// files `%0` under `UnnamedArgNums` rather than under `Name`, and
/// `parseFunctionType`'s rejection loop only asks `!Arg.Name.empty()` — so
/// only a `%name` trips `argument name invalid in function type`.
///
/// llvmkit rejected both spellings, which was stricter than upstream.
#[test]
fn a_function_type_accepts_a_numbered_argument_but_not_a_named_one() {
    let m = llvmkit_ir::Module::dynamic("function_type_argument_names");
    parse_into("%s = type i32 (i32 %0)\n", &m);

    assert_eq!(
        header_err("%s = type i32 (i32 %x)\n"),
        "argument name invalid in function type"
    );
    assert_eq!(
        header_err("%s = type i32 (i32 zeroext)\n"),
        "argument attributes invalid in function type"
    );
}

/// The two texts `parseArgumentList` reports for a malformed list, which
/// llvmkit had replaced with four per-site spellings — `'(' in function
/// declaration`, `'(' in function header`, `')' to close function
/// declaration`, `')' to close function header`.
#[test]
fn the_argument_list_delimiters_use_upstream_text() {
    assert_eq!(
        header_err("declare void @f[]\n"),
        "expected '(' in function argument list"
    );
    assert_eq!(
        header_err("define void @f[] {\nentry:\n  ret void\n}\n"),
        "expected '(' in function argument list"
    );
    assert_eq!(
        header_err("declare void @f(i32 %a\n"),
        "expected ')' at end of argument list"
    );
}

/// `parseFunctionHeader`'s redefinition arm. Upstream always creates a
/// **fresh** `Function`; only a forward reference is reused, so a repeated
/// `declare`, a repeated `define`, and a `declare` followed by a `define` are
/// all errors. llvmkit reused any function whose signature happened to match,
/// and accepted all three.
///
/// The two texts differ by namespace *and* by one `@`: a pre-existing
/// function is `invalid redefinition of function 'f'` (`M->getFunction`),
/// while any other named value is `redefinition of function '@f'`
/// (`M->getNamedValue`).
///
/// No `test/Assembler` fixture pins either — and none of the 500 both
/// declares and defines one function, which is the same fact from the other
/// side.
#[test]
fn a_function_redefinition_is_rejected() {
    let define = "define void @f() {\nentry:\n  ret void\n}\n";
    for src in [
        "declare void @f()\ndeclare void @f()\n".to_owned(),
        format!("{define}{define}"),
        format!("declare void @f()\n{define}"),
    ] {
        let src = src.as_str();
        assert_eq!(
            header_err(src),
            "invalid redefinition of function 'f'",
            "{src}"
        );
    }

    for src in [
        "@f = global i32 0\ndeclare void @f()\n",
        "@g = global i32 0\n@f = alias i32, ptr @g\ndeclare void @f()\n",
    ] {
        assert_eq!(header_err(src), "redefinition of function '@f'", "{src}");
    }
}

/// The case the redefinition arm must *not* catch: a function referenced
/// before it is defined. Upstream keeps it in `ForwardRefVals` and reuses the
/// placeholder; llvmkit keeps the provisional `Function` it minted at the
/// call site.
#[test]
fn a_forward_referenced_function_is_not_a_redefinition() {
    let m = llvmkit_ir::Module::dynamic("forward_referenced_function");
    parse_into(
        "define void @g() {\nentry:\n  call void @f()\n  ret void\n}\n\
         define void @f() {\nentry:\n  ret void\n}\n",
        &m,
    );
    let printed = format!("{m}");
    assert!(printed.contains("define void @f()"), "{printed}");
    assert!(printed.contains("call void @f()"), "{printed}");
}

/// `parseFunctionHeader` runs its clause chain as a fixed **sequence** of
/// single `EatIfPresent` guards, so the order is contractual and each clause
/// appears at most once. llvmkit looped over them instead, accepting any
/// order and any number of repeats.
///
/// `align` is the interesting one: it is legal *before* `section` because the
/// attribute loop parses it and `parseFunctionHeader` then moves it to the
/// alignment field ("As a hack, we allow function alignment to be initially
/// parsed as an attribute…", upstream's own comment), and legal again after
/// `comdat` as the clause. llvmkit used to exclude `align` from the attribute
/// loop entirely, which is invisible while the chain is order-free.
#[test]
fn the_header_clause_chain_is_a_fixed_sequence() {
    // In order: accepted.
    let m = llvmkit_ir::Module::dynamic("clause_order_ok");
    parse_into(
        "define void @f() section \"s\" align 8 gc \"g\" {\nentry:\n  ret void\n}\n",
        &m,
    );
    // `align` as an attribute, before `section`: also accepted, and it lands
    // in the alignment field rather than the attribute list.
    let m = llvmkit_ir::Module::dynamic("clause_order_align_attr");
    parse_into(
        "define void @f() align 8 section \"s\" {\nentry:\n  ret void\n}\n",
        &m,
    );
    let printed = format!("{m}");
    assert!(printed.contains("section \"s\""), "{printed}");
    assert!(printed.contains("align 8"), "{printed}");

    // Out of order: `section` is looked for before `gc`, so the trailing
    // `section` is left over and fails against the body's `{`.
    assert_eq!(
        header_err("define void @f() gc \"g\" section \"s\" {\nentry:\n  ret void\n}\n"),
        "expected '{' in function body"
    );
    // And a clause may not repeat.
    assert_eq!(
        header_err("define void @f() section \"a\" section \"b\" {\nentry:\n  ret void\n}\n"),
        "expected '{' in function body"
    );
}

/// `if (FuncAttrs.contains(Attribute::Builtin))` in
/// `LLParser::parseFunctionHeader`, anchored at `BuiltinLoc` — which is why
/// upstream threads that location out of `parseFnAttributeValuePairs` at all.
///
/// The attribute itself is real: a *call site* may carry `builtin`, so the
/// rejection cannot live in the attribute loop.
#[test]
fn builtin_is_not_a_function_attribute() {
    for src in [
        "declare void @f() builtin\n",
        "define void @f() builtin {\nentry:\n  ret void\n}\n",
    ] {
        assert_eq!(
            header_err(src),
            "'builtin' attribute not valid on function",
            "{src}"
        );
    }

    // The call-site spelling stays legal.
    let m = llvmkit_ir::Module::dynamic("builtin_call_site");
    parse_into(
        "declare void @g()\ndefine void @f() {\nentry:\n  call void @g() builtin\n  ret void\n}\n",
        &m,
    );
}

/// `if (PAL.hasParamAttr(0, Attribute::StructRet) && !RetType->isVoidTy())`,
/// reported at `RetTypeLoc`. Parameter **0** only, which is the whole rule.
#[test]
fn an_sret_first_argument_forces_a_void_return() {
    assert_eq!(
        header_err("declare i32 @f(ptr sret(i32) %p)\n"),
        "functions with 'sret' argument must return void"
    );

    // Void return: fine. `sret` on a later parameter: not this rule.
    let m = llvmkit_ir::Module::dynamic("sret_ok");
    parse_into("declare void @f(ptr sret(i32) %p)\n", &m);
    let m = llvmkit_ir::Module::dynamic("sret_second_param");
    parse_into("declare i32 @f(ptr %a, ptr sret(i32) %p)\n", &m);
}

/// `parseFunctionHeader`'s tail, reached only when `IsDefine` is false: a
/// `blockaddress` naming a function that turns out to be a *declaration* can
/// never be satisfied. Reported at the reference, not at the declaration.
#[test]
fn a_blockaddress_may_not_name_a_declaration() {
    assert_eq!(
        header_err("@a = global ptr blockaddress(@f, %bb)\ndeclare void @f()\n"),
        "cannot take blockaddress inside a declaration"
    );
}

/// The four texts `LLParser::parseFunctionBody` and `parseBasicBlock` own,
/// three of them ported from their upstream fixtures.
///
/// `function body requires at least one basic block` had no llvmkit
/// equivalent at all: its loop simply broke on `}`, so `define void @f() { }`
/// parsed as a body with zero blocks.
#[test]
fn the_function_body_frame_matches_upstream_text() {
    // `test/Assembler/align-param-attr-error1.ll` and
    // `test/Assembler/mustprogress-parse-error-2.ll` both pin the brace.
    for fixture in [
        include_bytes!("fixtures/upstream/align-param-attr-error1.ll").as_slice(),
        include_bytes!("fixtures/upstream/mustprogress-parse-error-2.ll").as_slice(),
    ] {
        let m = llvmkit_ir::Module::dynamic("function_body_brace");
        let err = Parser::new(fixture, &m)
            .expect("lexer primes")
            .parse_module()
            .expect_err("fixture is rejected")
            .to_string();
        assert_eq!(err, "expected '{' in function body");
    }

    // `test/Assembler/2004-03-30-UnclosedFunctionCrash.ll`
    let m = llvmkit_ir::Module::dynamic("unclosed_function");
    let err = Parser::new(
        include_bytes!("fixtures/upstream/2004-03-30-UnclosedFunctionCrash.ll").as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect_err("fixture is rejected")
    .to_string();
    assert_eq!(err, "found end of file when expecting more instructions");

    // `test/Assembler/2003-11-24-SymbolTableCrash.ll`. Note upstream spells
    // the name **without** a `%`, unlike its `redefinition of ...` family.
    let m = llvmkit_ir::Module::dynamic("symbol_table_crash");
    let err = Parser::new(
        include_bytes!("fixtures/upstream/2003-11-24-SymbolTableCrash.ll").as_slice(),
        &m,
    )
    .expect("lexer primes")
    .parse_module()
    .expect_err("fixture is rejected")
    .to_string();
    assert_eq!(err, "multiple definition of local value named 'tmp.1'");

    // No upstream fixture pins the empty body; the routine is the anchor.
    assert_eq!(
        header_err("define void @f() {\n}\n"),
        "function body requires at least one basic block"
    );
    assert_eq!(
        header_err("define void @f() {\nuselistorder ptr @f, { 1, 0 }\n}\n"),
        "function body requires at least one basic block"
    );
}

/// `parseFunctionBody`'s two loops are ordered: every basic block, *then*
/// every `uselistorder` directive. llvmkit ran one loop that accepted either
/// at any point, so a block after a directive parsed.
#[test]
fn uselistorder_directives_come_after_every_block() {
    let m = llvmkit_ir::Module::dynamic("uselistorder_placement");
    parse_into(
        "define i32 @f(i32 %a) {\nentry:\n  %b = add i32 %a, %a\n  ret i32 %b\n\
         uselistorder i32 %a, { 1, 0 }\n}\n",
        &m,
    );

    // Once the directives start, the only things left are more directives or
    // the `}` — `parseUseListOrder`'s own `parseToken` reports the label.
    assert_eq!(
        header_err(
            "define i32 @f(i32 %a) {\nentry:\n  %b = add i32 %a, %a\n  ret i32 %b\n\
             uselistorder i32 %a, { 1, 0 }\nsecond:\n  ret i32 0\n}\n"
        ),
        "expected 'uselistorder'"
    );
}

/// `PerFunctionState::defineBB` reaches a named block through
/// `getVal(Name, LabelTy)`, so blocks and local values share one namespace: a
/// label whose name is already an instruction result cannot be created.
///
/// llvmkit keeps blocks in a map of their own, so it used to create *both* —
/// a value `%x` and a block `%x` in the same function.
///
/// The label must carry no forward reference: `br label %x` would reach
/// `getVal` first and fail there instead, with
/// `'%x' defined with type 'i32' but expected 'label'`.
///
/// The numbered twin (`unable to create block numbered '<N>'`) is not tested:
/// `defineBB` runs `checkValueID` first, so a numbered label that collides
/// has already failed with `label expected to be numbered 'N' or greater`,
/// and no `test/Assembler` fixture reaches it.
#[test]
fn a_block_may_not_take_a_local_value_name() {
    assert_eq!(
        header_err(
            "define void @f() {\nentry:\n  %x = add i32 0, 0\n  ret void\nx:\n  ret void\n}\n"
        ),
        "unable to create block named 'x'"
    );
}

/// `parseFunctionHeader`'s argument-naming loop: upstream sets each name and
/// notices when the symbol table renamed it. llvmkit installed the names
/// without checking, so the second `%x` silently won.
#[test]
fn a_repeated_argument_name_is_rejected() {
    for src in [
        "declare void @f(i32 %x, i32 %x)\n",
        "define void @f(i32 %x, i32 %x) {\nentry:\n  ret void\n}\n",
    ] {
        assert_eq!(header_err(src), "redefinition of argument '%x'", "{src}");
    }
}

/// `parseFunctionHeader`'s `tokError("expected function name")`. llvmkit
/// appended ` after return type`, which upstream never says.
///
/// Note what `declare void ()` is *not*: `parseType`'s suffix loop turns
/// `void (` into a function **type**, so that spelling is
/// `invalid function return type` — a function returning a function — and
/// never reaches the name check at all.
#[test]
fn a_missing_function_name_uses_upstream_text() {
    assert_eq!(header_err("declare void 42\n"), "expected function name");
    assert_eq!(
        header_err("declare void ()\n"),
        "invalid function return type"
    );
}
