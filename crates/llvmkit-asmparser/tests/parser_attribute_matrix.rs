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
fn dso_local_ifunc_parses_and_prints() {
    // Ifuncs parse and print the specifier; full round-trip is blocked by a
    // pre-existing, dso-independent gap (below).
    let m = parse_dynamic("declare ptr @r()\n@i = dso_local ifunc i32 (i32), ptr @r\n")
        .expect("dso_local ifunc");
    let printed = format!("{m}");
    assert!(printed.contains("dso_local ifunc"), "{printed}");
}

/// Pre-existing gap, pinned so it cannot be forgotten: the printer emits
/// ifuncs before function declarations, and the parser cannot forward-reference
/// an ifunc resolver, so a printed module with an ifunc whose resolver is a
/// declared function does not re-parse. Nothing to do with attributes — the
/// snippet below contains none. Recorded in `docs/future-work.md`; when the
/// parser learns deferred alias/ifunc targets, this test flips to asserting
/// the round-trip and the ifunc case above joins the matrix.
#[test]
fn known_gap_ifunc_forward_resolver_round_trip() {
    let m = parse_dynamic("declare ptr @r()\n@i = ifunc i32 (i32), ptr @r\n").expect("parse");
    let printed = format!("{m}");
    let Err(err) = parse_dynamic(printed.as_str()) else {
        panic!("if this now round-trips, delete this test and un-gap the matrix");
    };
    assert!(format!("{err}").contains('r'), "{err}");
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
    // `dead_on_unwind` may or may not be modeled; strip to the M0 target set.
    let src = src.replace("dead_on_unwind ", "");
    let m = parse_dynamic(src.as_str()).expect("byval/sret C shape parses");
    let printed = format!("{m}");
    assert!(printed.contains("byval(%struct.Point)"), "{printed}");
    assert!(printed.contains("sret(%struct.Point)"), "{printed}");
    parse_dynamic(printed.as_str()).expect("round-trip");
}
