//! The attribute keyword matrix — the probe that found Milestone 0's gaps,
//! landed as a test so the next missing keyword fails CI rather than a user's
//! first attempt (`ROADMAP.md`, Milestone 0 acceptance criteria).
//!
//! Every case asserts three things: the snippet parses, the printed module
//! still contains the attribute's canonical spelling (no silent drop), and
//! the printed module re-parses (round-trip).

use llvmkit_asmparser::parse_dynamic;

fn parse_print_reparse(label: &str, src: &str, expect_printed: &str) {
    let m = parse_dynamic(src).unwrap_or_else(|e| panic!("{label}: parse failed: {e}\n{src}"));
    let printed = format!("{m}");
    assert!(
        printed.contains(expect_printed),
        "{label}: printed module dropped {expect_printed:?}:\n{printed}"
    );
    parse_dynamic(printed.as_str())
        .unwrap_or_else(|e| panic!("{label}: round-trip failed: {e}\n--- printed ---\n{printed}"));
}

// -- function attributes (attribute-group position) --------------------------

#[test]
fn function_attributes_previously_missing() {
    for kw in [
        "uwtable",
        "norecurse",
        "hot",
        "inlinehint",
        "sanitize_address",
        "ssp",
        "sspstrong",
        "sspreq",
        "nonlazybind",
        "minsize",
    ] {
        let src = format!("define void @f() #0 {{ ret void }}\nattributes #0 = {{ {kw} }}\n");
        parse_print_reparse(kw, &src, kw);
    }
}

#[test]
fn function_attributes_already_supported_still_work() {
    for kw in [
        "noinline",
        "nounwind",
        "optnone",
        "readnone",
        "willreturn",
        "mustprogress",
        "nofree",
        "nosync",
        "cold",
        "noreturn",
        "speculatable",
        "alwaysinline",
        "optsize",
        "convergent",
        "nocallback",
        "strictfp",
        "noduplicate",
    ] {
        let src = format!("define void @f() #0 {{ ret void }}\nattributes #0 = {{ {kw} }}\n");
        // The legacy memory keywords normalise to the `memory(...)` form on
        // print; everything else keeps its spelling.
        let expect = match kw {
            "readnone" => "memory(none)",
            "readonly" => "memory(read)",
            _ => kw,
        };
        parse_print_reparse(kw, &src, expect);
    }
}

#[test]
fn uwtable_kind_grammar() {
    // Bare uwtable is the async kind and prints bare — never `uwtable(2)`.
    let m = parse_dynamic("define void @f() #0 { ret void }\nattributes #0 = { uwtable }\n")
        .expect("bare uwtable");
    let printed = format!("{m}");
    assert!(printed.contains("uwtable"), "{printed}");
    assert!(
        !printed.contains("uwtable("),
        "bare form stays bare: {printed}"
    );

    parse_print_reparse(
        "uwtable(sync)",
        "define void @f() #0 { ret void }\nattributes #0 = { uwtable(sync) }\n",
        "uwtable(sync)",
    );

    // `uwtable(async)` normalises to the bare spelling, as upstream prints it.
    let m = parse_dynamic("define void @f() #0 { ret void }\nattributes #0 = { uwtable(async) }\n")
        .expect("uwtable(async)");
    let printed = format!("{m}");
    assert!(!printed.contains("uwtable(async)"), "{printed}");
    assert!(printed.contains("uwtable"), "{printed}");
}

// -- parameter attributes ----------------------------------------------------

#[test]
fn parameter_attributes_previously_missing() {
    let typed = [
        ("byval", "byval(%s)"),
        ("sret", "sret(%s)"),
        ("byref", "byref(%s)"),
        ("inalloca", "inalloca(%s)"),
        ("elementtype", "elementtype(%s)"),
    ];
    for (label, attr) in typed {
        let src = format!("%s = type {{ i32 }}\ndefine void @f(ptr {attr} %p) {{ ret void }}\n");
        parse_print_reparse(label, &src, attr);
    }

    let plain = [
        ("dereferenceable", "dereferenceable(8)"),
        ("dereferenceable_or_null", "dereferenceable_or_null(16)"),
        ("inreg", "inreg"),
        ("nest", "nest"),
        ("swiftself", "swiftself"),
    ];
    for (label, attr) in plain {
        let src = format!("define void @f(ptr {attr} %p) {{ ret void }}\n");
        parse_print_reparse(label, &src, attr);
    }
}

/// `captures(none)` prints back as itself, not as `nocapture`.
///
/// This test used to assert the opposite half too — that any component other
/// than `none` must *fail* to parse — which pinned llvmkit's limitation
/// rather than upstream's behaviour. The full grammar is now modelled, so the
/// full component set is covered by `captures_components_round_trip` and the
/// half that asserted a gap is gone.
#[test]
fn captures_none_round_trips_as_itself() {
    let m = parse_dynamic("define void @f(ptr captures(none) %p) { ret void }\n")
        .expect("captures(none)");
    let printed = format!("{m}");
    assert!(printed.contains("captures(none)"), "{printed}");
    assert!(!printed.contains("nocapture"), "{printed}");
    parse_dynamic(printed.as_str()).expect("round-trip");
}

#[test]
fn alignstack_paren_form_round_trips() {
    parse_print_reparse(
        "alignstack",
        "define void @f() #0 { ret void }
attributes #0 = { alignstack(16) }
",
        "alignstack(16)",
    );
}

#[test]
fn return_attribute_dereferenceable() {
    parse_print_reparse(
        "ret dereferenceable",
        "define dereferenceable(4) ptr @f(ptr %p) { ret ptr %p }\n",
        "dereferenceable(4)",
    );
}

// -- runtime preemption specifiers on global objects -------------------------

#[test]
fn dso_specifiers_on_global_objects() {
    let cases = [
        (
            "global",
            "@g = dso_local global i32 0\n",
            "dso_local global",
        ),
        (
            "constant",
            "@g = dso_local constant i32 7\n",
            "dso_local constant",
        ),
        (
            "preemptable global",
            "@g = dso_preemptable global i32 0\n",
            "dso_preemptable global",
        ),
        (
            "linkage + dso",
            "@g = internal dso_local global i32 0\n",
            "internal dso_local global",
        ),
        (
            "dso + unnamed_addr",
            "@g = dso_local unnamed_addr global i32 0\n",
            "dso_local unnamed_addr global",
        ),
        (
            "alias",
            "@t = global i32 0\n@a = dso_local alias i32, ptr @t\n",
            "dso_local alias",
        ),
        (
            "define",
            "define dso_local void @f() { ret void }\n",
            "dso_local void @f",
        ),
        (
            "declare",
            "declare dso_local void @f()\n",
            "dso_local void @f",
        ),
    ];
    for (label, src, expect) in cases {
        parse_print_reparse(label, src, expect);
    }
}

#[test]
fn dso_local_ifunc_round_trips() {
    parse_print_reparse(
        "dso_local ifunc",
        "declare ptr @r()\n@i = dso_local ifunc i32 (i32), ptr @r\n",
        "dso_local ifunc",
    );
}

/// Aliases and ifuncs may name a target declared later in the file — which is
/// exactly what the printer produces, since it emits them before function
/// declarations. A forward target becomes a null placeholder patched at end of
/// module (the mechanism `personality` already used for the same ordering
/// problem), so printed modules round-trip. A target that is never defined at
/// all must still be an error.
#[test]
fn alias_and_ifunc_forward_targets() {
    parse_print_reparse(
        "ifunc forward resolver",
        "declare ptr @r()\n@i = ifunc i32 (i32), ptr @r\n",
        "ifunc",
    );
    parse_print_reparse(
        "alias forward target",
        "@a = alias i32, ptr @t\n@t = global i32 0\n",
        "alias",
    );
    parse_print_reparse(
        "alias to later function",
        "@a = alias i32 (i32), ptr @f\ndefine i32 @f(i32 %x) { ret i32 %x }\n",
        "alias",
    );
    // Backward references keep working.
    parse_print_reparse(
        "alias backward target",
        "@t = global i32 0\n@a = alias i32, ptr @t\n",
        "alias",
    );

    let Err(err) = parse_dynamic("@a = alias i32, ptr @nope\n") else {
        panic!("a target that is never defined must not parse");
    };
    assert!(format!("{err}").contains("nope"), "{err}");
}

// -- whole-program shapes: what clang actually emits -------------------------

#[test]
fn clang_o0_shape_parses_verifies_round_trips() {
    let src = r#"
; ModuleID = 'a.c'
source_filename = "a.c"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

define dso_local i32 @add(i32 noundef %a, i32 noundef %b) #0 {
entry:
  %a.addr = alloca i32, align 4
  %b.addr = alloca i32, align 4
  store i32 %a, ptr %a.addr, align 4
  store i32 %b, ptr %b.addr, align 4
  %0 = load i32, ptr %a.addr, align 4
  %1 = load i32, ptr %b.addr, align 4
  %add = add nsw i32 %0, %1
  ret i32 %add
}

attributes #0 = { noinline nounwind optnone uwtable "frame-pointer"="all" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "target-cpu"="x86-64" }

!llvm.module.flags = !{!0, !1, !2}
!llvm.ident = !{!3}

!0 = !{i32 1, !"wchar_size", i32 4}
!1 = !{i32 8, !"PIC Level", i32 2}
!2 = !{i32 7, !"uwtable", i32 2}
!3 = !{!"clang version 18.1.0"}
"#;
    let m = parse_dynamic(src).expect("clang -O0 shape parses");
    let printed = format!("{m}");
    parse_dynamic(printed.as_str()).expect("clang -O0 round-trips");
    m.verify().expect("clang -O0 verifies");
}

#[test]
fn clang_o2_shape_parses_verifies_round_trips() {
    let src = r#"
target triple = "x86_64-unknown-linux-gnu"

@counter = dso_local local_unnamed_addr global i32 0, align 4
@.str = private unnamed_addr constant [6 x i8] c"hello\00", align 1

declare void @use(i32) local_unnamed_addr

define dso_local i32 @sum_to(i32 noundef %n) local_unnamed_addr #0 {
entry:
  %cmp5 = icmp sgt i32 %n, 0
  br i1 %cmp5, label %for.body, label %for.end

for.body:                                         ; preds = %entry, %for.body
  %i.06 = phi i32 [ %inc, %for.body ], [ 0, %entry ]
  %acc.07 = phi i32 [ %add, %for.body ], [ 0, %entry ]
  %add = add nuw nsw i32 %acc.07, %i.06
  %inc = add nuw nsw i32 %i.06, 1
  %exitcond.not = icmp eq i32 %inc, %n
  br i1 %exitcond.not, label %for.end, label %for.body

for.end:                                          ; preds = %for.body, %entry
  %acc.0.lcssa = phi i32 [ 0, %entry ], [ %add, %for.body ]
  ret i32 %acc.0.lcssa
}

attributes #0 = { nofree norecurse nosync nounwind memory(none) }
"#;
    let m = parse_dynamic(src).expect("clang -O2 shape parses");
    let printed = format!("{m}");
    parse_dynamic(printed.as_str()).expect("clang -O2 round-trips");
    m.verify().expect("clang -O2 verifies");
}

// -- struct-by-value C code: the byval/sret shape ----------------------------

#[test]
fn struct_by_value_c_shape() {
    let src = r#"
%struct.Point = type { i32, i32 }

define dso_local void @consume(ptr noundef byval(%struct.Point) align 4 %p) {
entry:
  ret void
}

define dso_local void @produce(ptr dead_on_unwind noalias writable sret(%struct.Point) align 4 %agg.result) {
entry:
  ret void
}
"#;
    let m = parse_dynamic(src).expect("byval/sret C shape parses");
    let printed = format!("{m}");
    assert!(printed.contains("byval(%struct.Point)"), "{printed}");
    assert!(printed.contains("sret(%struct.Point)"), "{printed}");
    assert!(printed.contains("dead_on_unwind"), "{printed}");
    parse_dynamic(printed.as_str()).expect("round-trip");
}

/// `captures(...)` in full: the `CaptureComponents` lattice, the `ret:`
/// sublocation, and `operator<<(raw_ostream &, CaptureInfo)`'s printing —
/// which is what `Attribute::getAsString` emits for this attribute, parens
/// and keyword included.
///
/// The printed forms are the ones `test/Assembler/captures.ll` pins. Note
/// `captures(address, ret: none)`: a `ret:` bucket that differs from the
/// other bucket is always printed, even when it is empty, while a missing
/// `ret:` means the two buckets are *equal*, not that the return captures
/// nothing.
#[test]
fn captures_components_round_trip() {
    for (spelled, printed) in [
        ("captures(none)", "captures(none)"),
        ("captures(address)", "captures(address)"),
        ("captures(address_is_null)", "captures(address_is_null)"),
        (
            "captures(address, provenance)",
            "captures(address, provenance)",
        ),
        (
            "captures(address, read_provenance)",
            "captures(address, read_provenance)",
        ),
        // `ret:` may appear first, or later, and swallows everything after it.
        (
            "captures(ret: address, provenance)",
            "captures(ret: address, provenance)",
        ),
        (
            "captures(address_is_null, ret: address, provenance)",
            "captures(address_is_null, ret: address, provenance)",
        ),
        // The `none` guard is per bucket, so this is legal.
        (
            "captures(address, ret: none)",
            "captures(address, ret: none)",
        ),
        // Components accumulate with `|=`, so a repeat collapses.
        ("captures(address, address)", "captures(address)"),
        // `address` subsumes `address_is_null` — the lattice, not four flags.
        ("captures(address_is_null, address)", "captures(address)"),
    ] {
        parse_print_reparse(
            spelled,
            &format!("define void @f(ptr {spelled} %p) {{ ret void }}\n"),
            printed,
        );
    }
}

/// `parseCapturesAttr`'s own diagnostics.
#[test]
fn captures_diagnostics_match_upstream_text() {
    fn parse_err(spelled: &str) -> String {
        parse_dynamic(format!("define void @f(ptr {spelled} %p) {{ ret void }}\n"))
            .expect_err("captures attribute is rejected")
            .to_string()
    }

    assert_eq!(
        parse_err("captures(ret: address, ret: provenance)"),
        "duplicate 'ret' location"
    );
    assert_eq!(
        parse_err("captures(address, none)"),
        "cannot use 'none' with other component"
    );
    assert_eq!(
        parse_err("captures(none, address)"),
        "cannot use 'none' with other component"
    );
    // `captures(bogus)` would pin the same message, but a word matching no
    // keyword never reaches the parser: llvmkit's lexer raises
    // `unknown keyword 'bogus'` where upstream returns a silent error token.
    // Blocked on the same lexer re-layering as three splits of
    // `memory-attribute-errors.ll`. `captures()` reaches the arm instead,
    // because `)` *is* a token the parser sees.
    //
    // `captures()` is not an empty set: the loop demands a component first.
    assert_eq!(
        parse_err("captures()"),
        "expected one of 'none', 'address', 'address_is_null', 'provenance' or 'read_provenance'"
    );
    assert_eq!(parse_err("captures(address"), "expected ',' or ')'");
    assert_eq!(parse_err("captures(ret address)"), "expected ':'");
}

/// `range(iN lo, hi)` accepts three shapes worth pinning: an ordinary range,
/// the one legal degenerate form `range(i8 0, 0)` — legal precisely because
/// the empty-set check exempts zero, which is why
/// `test/Assembler/range-attribute-invalid-range.ll` writes `1, 1` and not
/// `0, 0` — and a **wrapped** range with `lower > upper`, which
/// `test/Verifier/range-attr.ll` pins as parsing cleanly (its complaint is
/// about the annotated value's type, not the range).
#[test]
fn range_attribute_shapes_round_trip() {
    for spelled in [
        "range(i8 0, 64)",
        "range(i8 -1, 0)",
        "range(i8 0, 0)",
        "range(i8 1, 0)",
    ] {
        parse_print_reparse(
            spelled,
            &format!("define void @f(i8 {spelled} %a) {{ ret void }}\n"),
            spelled,
        );
    }
}

/// `LLParser::parseRangeAttr`'s seven diagnostics, verbatim.
///
/// Two are ported fixtures: `test/Assembler/range-attribute-invalid-range.ll`
/// (`range(i8 1, 1)`) and `test/Assembler/range-attribute-invalid-type.ll`
/// (`range(<4 x i32> 0, 0)`, which pins that `Type::isIntegerTy` is false for
/// a *vector* of integers).
///
/// `integer is too large for the bit width of specified type` has **no**
/// upstream fixture — nothing in the tree emits it — so its three cases are
/// derived from the `ParseAPSInt` lambda together with
/// `APSInt::APSInt(StringRef)` and `LLLexer::lexIdentifier`'s `[us]0x` rule.
/// The third is the subtle one: an all-zero hex literal keeps its full
/// syntactic width, because the active-bit trim is guarded by `activeBits > 0`.
#[test]
fn range_diagnostics_match_upstream_text() {
    fn parse_err(spelled: &str) -> String {
        parse_dynamic(format!("define void @f(i8 {spelled} %a) {{ ret void }}\n"))
            .expect_err("range attribute is rejected")
            .to_string()
    }

    assert_eq!(parse_err("range i8 0, 4"), "expected '('");
    assert_eq!(
        parse_err("range(<4 x i32> 0, 0)"),
        "the range must have integer type!"
    );
    assert_eq!(parse_err("range(i8 1.5, 4)"), "expected integer");
    assert_eq!(parse_err("range(i8 0 4)"), "expected ','");
    assert_eq!(
        parse_err("range(i8 1, 1)"),
        "the range represent the empty set but limits aren't 0!"
    );
    assert_eq!(parse_err("range(i8 0, 4 %a)"), "expected ')'");

    for spelled in [
        // 300 needs nine active bits.
        "range(i8 300, 0)",
        // -255 needs nine significant bits, and llvmkit used to accept this
        // and silently wrap the bound to 1.
        "range(i8 -255, 0)",
        // Eighteen zero digits: no active bits, so no trim, so 72 bits wide.
        "range(i8 u0x000000000000000000, 1)",
    ] {
        assert_eq!(
            parse_err(spelled),
            "integer is too large for the bit width of specified type",
            "{spelled}"
        );
    }
}

/// Ports the `@initializes` function of `test/Bitcode/attributes.ll`, whose
/// `; CHECK: define void @initializes(ptr initializes((-4, 0), (4, 8)) %a)`
/// is the only fixture in the tree that pins this attribute's printed form.
/// It fixes the `, ` between ranges and inside each range, the absence of a
/// space after the keyword, and signed rendering of a negative bound.
#[test]
fn initializes_round_trips() {
    parse_print_reparse(
        "initializes",
        "define void @initializes(ptr initializes((-4, 0), (4, 8)) %a) {\n  ret void\n}\n",
        "initializes((-4, 0), (4, 8))",
    );
}

/// Ports all ten splits of `test/Assembler/initializes-attribute-invalid.ll`,
/// each asserting the exact text its `FileCheck` prefix pins.
///
/// Two of them are subtler than they look: `initializes()` fails on the
/// **inner** `(` — the outer one was already consumed and the do-loop demands
/// at least one range — and `initializes((0, 4) (8, 12))` reports a missing
/// `)` rather than a missing `,`, because the list separator is read with
/// `EatIfPresent` and its absence simply ends the loop.
#[test]
fn initializes_diagnostics_match_upstream_text() {
    fn parse_err(spelled: &str) -> String {
        parse_dynamic(format!(
            "define void @foo(ptr {spelled} %a) {{\n  ret void\n}}\n"
        ))
        .expect_err("initializes attribute is rejected")
        .to_string()
    }

    // OUTER-LEFT
    assert_eq!(parse_err("initializes 0, 4"), "expected '('");
    // INNER-LEFT
    assert_eq!(parse_err("initializes(0, 4"), "expected '('");
    // INNER-RIGHT
    assert_eq!(parse_err("initializes((0, 4"), "expected ')'");
    // OUTER-RIGHT
    assert_eq!(parse_err("initializes((0, 4)"), "expected ')'");
    // INTEGER
    assert_eq!(parse_err("initializes((0.5, 4))"), "expected integer");
    // LOWER-EQUAL-UPPER
    assert_eq!(
        parse_err("initializes((4, 4))"),
        "the range should not represent the full or empty set!"
    );
    // INNER-COMMA
    assert_eq!(parse_err("initializes((0 4))"), "expected ','");
    // OUTER-COMMA
    assert_eq!(parse_err("initializes((0, 4) (8, 12))"), "expected ')'");
    // EMPTY1
    assert_eq!(parse_err("initializes()"), "expected '('");
    // EMPTY2
    assert_eq!(parse_err("initializes(())"), "expected integer");
}

/// Ports all five splits of `test/Verifier/initializes-attr.ll`.
///
/// The file is misfiled upstream: every case is rejected by `llvm-as`'s
/// *parser*, through `ConstantRangeList::getConstantRangeList`, and never
/// reaches the verifier — which is why they are asserted here as parse
/// errors. `overlapping1` is the one that matters most: `(0, 4), (4, 8)`
/// merely *touch*, and `isOrderedRanges` compares with `sle`, so adjacency
/// is rejected too.
#[test]
fn initializes_range_list_invariants_match_upstream_text() {
    fn parse_err(spelled: &str) -> String {
        parse_dynamic(format!(
            "define void @foo(ptr {spelled} %a) {{\n  ret void\n}}\n"
        ))
        .expect_err("initializes range list is rejected")
        .to_string()
    }

    for spelled in [
        // lower_greater_than_upper1
        "initializes((4, 0))",
        // lower_greater_than_upper2
        "initializes((0, 4), (8, 6))",
        // descending_order
        "initializes((8, 12), (0, 4))",
        // overlapping1 — adjacency, not overlap
        "initializes((0, 4), (4, 8))",
        // overlapping2
        "initializes((0, 4), (2, 8))",
    ] {
        assert_eq!(
            parse_err(spelled),
            "Invalid (unordered or overlapping) range list",
            "{spelled}"
        );
    }
}

/// The three position diagnostics, from `Attribute::canUseAsFnAttr` and its
/// two siblings as `parseFnAttributeValuePairs` and
/// `parseOptionalParamOrReturnAttrs` ask them.
///
/// `align` is the exemption upstream calls out by name: it is
/// `[ParamAttr, RetAttr]` in `Attributes.td`, yet the function loop accepts it
/// anyway — "as a hack, we allow function alignment to be initially parsed as
/// an attribute … and later moved to the alignment field."
#[test]
fn attributes_are_rejected_outside_their_declared_positions() {
    fn parse_err(src: &str) -> String {
        parse_dynamic(src)
            .expect_err("attribute is in the wrong position")
            .to_string()
    }

    // `noalias` is `[ParamAttr, RetAttr]`, never a function attribute.
    assert_eq!(
        parse_err("define void @f() #0 { ret void }\nattributes #0 = { noalias }\n"),
        "this attribute does not apply to functions"
    );
    // `alwaysinline` is `[FnAttr]` only.
    assert_eq!(
        parse_err("define void @f(ptr alwaysinline %p) { ret void }\n"),
        "this attribute does not apply to parameters"
    );
    assert_eq!(
        parse_err("define alwaysinline ptr @f() { ret ptr null }\n"),
        "this attribute does not apply to return values"
    );
    // `byval` is `[ParamAttr]`: legal on a parameter, not on a return value.
    assert_eq!(
        parse_err("%s = type { i32 }\ndefine byval(%s) ptr @f() { ret ptr null }\n"),
        "this attribute does not apply to return values"
    );

    // The `align` hack, both spellings.
    parse_print_reparse(
        "align in an attribute group",
        "define void @f() #0 { ret void }\nattributes #0 = { align 8 }\n",
        "align 8",
    );
    parse_print_reparse(
        "align on a parameter",
        "define void @f(ptr align 8 %p) { ret void }\n",
        "align 8",
    );
}

/// The three attributes whose argument needed a grammar of its own. Each
/// asserts the spelling `Attribute::getAsString` produces — note that all
/// three write their comma with no following space, and that `vscale_range`
/// always prints two arguments, using `0` for an unbounded maximum.
#[test]
fn the_argument_carrying_function_attributes_round_trip() {
    for (spelled, printed) in [
        ("allocsize(0)", "allocsize(0)"),
        ("allocsize(0, 1)", "allocsize(0,1)"),
        ("vscale_range(1, 16)", "vscale_range(1,16)"),
        // A missing maximum defaults to the *minimum*, not to unbounded.
        ("vscale_range(4)", "vscale_range(4,4)"),
        ("allockind(\"alloc\")", "allockind(\"alloc\")"),
        ("allockind(\"alloc,zeroed\")", "allockind(\"alloc,zeroed\")"),
        // `getAsString` emits the kinds in declaration order, whatever order
        // the source wrote them in.
        ("allockind(\"zeroed,alloc\")", "allockind(\"alloc,zeroed\")"),
    ] {
        parse_print_reparse(
            spelled,
            &format!("define void @f() #0 {{ ret void }}\nattributes #0 = {{ {spelled} }}\n"),
            printed,
        );
    }
}

/// `preallocated(T)` is a `TypeAttr`, so it takes the same production as
/// `byval` / `sret` / `elementtype`. Its `Attributes.td` def declares *both*
/// `FnAttr` and `ParamAttr` — the only type attribute that does — so both
/// positions are asserted.
#[test]
fn preallocated_is_a_type_attribute_in_both_positions() {
    parse_print_reparse(
        "preallocated param",
        "%s = type { i32 }\ndefine void @f(ptr preallocated(%s) %p) { ret void }\n",
        "preallocated(%s)",
    );
    parse_print_reparse(
        "preallocated fn",
        "%s = type { i32 }\ndefine void @f() #0 { ret void }\nattributes #0 = { preallocated(%s) }\n",
        "preallocated(%s)",
    );
}

/// The thirty-nine plain enum attributes wired in one sweep: every remaining
/// `EnumAttr` in `Attributes.td` that the lexer already tokenised and
/// `attr_kind_for_keyword` — upstream's `tokenToAttribute` — did not know.
/// Each is probed in the position its `.td` def declares.
///
/// `test/Bitcode/attributes.ll` is the upstream fixture that exercises these;
/// its RUN line is `llvm-as | llvm-dis | FileCheck`, a pure assembler
/// round-trip, which is the same three assertions `parse_print_reparse`
/// makes. It lives under `test/Bitcode` only because that is where LLVM keeps
/// its full-attribute-surface round-trip.
#[test]
fn every_remaining_plain_enum_attribute_round_trips() {
    const FUNCTION_ATTRIBUTES: &[&str] = &[
        "builtin",
        "coro_elide_safe",
        "disable_sanitizer_instrumentation",
        "coro_only_destroy_when_complete",
        "fn_ret_thunk_extern",
        "hybrid_patchable",
        "jumptable",
        "naked",
        "nobuiltin",
        "nocf_check",
        "nodivergencesource",
        "noimplicitfloat",
        "noprofile",
        "noredzone",
        "nosanitize_bounds",
        "nosanitize_coverage",
        "null_pointer_is_valid",
        "optdebug",
        "optforfuzzing",
        "presplitcoroutine",
        "returns_twice",
        "safestack",
        "sanitize_alloc_token",
        "sanitize_hwaddress",
        "sanitize_memory",
        "sanitize_memtag",
        "sanitize_numerical_stability",
        "sanitize_realtime",
        "sanitize_realtime_blocking",
        "sanitize_thread",
        "sanitize_type",
        "shadowcallstack",
        "skipprofile",
        "speculative_load_hardening",
    ];
    const PARAMETER_ATTRIBUTES: &[&str] = &[
        "allocalign",
        "allocptr",
        "dead_on_return",
        "dead_on_unwind",
        "noext",
        "swiftasync",
        "swifterror",
    ];

    for keyword in FUNCTION_ATTRIBUTES {
        parse_print_reparse(
            keyword,
            &format!("define void @f() #0 {{ ret void }}\nattributes #0 = {{ {keyword} }}\n"),
            keyword,
        );
    }
    for keyword in PARAMETER_ATTRIBUTES {
        parse_print_reparse(
            keyword,
            &format!("define void @f(ptr {keyword} %p) {{ ret void }}\n"),
            keyword,
        );
    }
}
