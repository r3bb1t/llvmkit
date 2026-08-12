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

#[test]
fn captures_none_maps_to_the_modeled_form() {
    // `captures(none)` is LLVM 21+'s spelling of the fact llvmkit models as
    // `nocapture`; accepting it must not silently drop the fact.
    let m = parse_dynamic("define void @f(ptr captures(none) %p) { ret void }\n")
        .expect("captures(none)");
    let printed = format!("{m}");
    assert!(
        printed.contains("nocapture") || printed.contains("captures(none)"),
        "capture fact dropped: {printed}"
    );
    parse_dynamic(printed.as_str()).expect("round-trip");

    // Other components are a pinpointed error, never a silent drop.
    let Err(err) = parse_dynamic("define void @f(ptr captures(address) %p) { ret void }\n") else {
        panic!("unsupported captures component must not parse");
    };
    assert!(format!("{err}").contains("captures"), "{err}");
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
