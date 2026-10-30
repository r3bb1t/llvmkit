//! The FileCheck subset in `tests/support/mod.rs` catches what it claims to.
//!
//! A ported `CHECK` block is only as strong as the harness reading it, and a
//! harness that accepted everything would make every port pass. Each test here
//! is a failure the harness must raise, with its positive control, and each is
//! llvmkit-specific: it pins this crate's own subset, against the
//! `lib/FileCheck/FileCheck.cpp` routine its doc names.

pub mod support;

use support::{Check, check_directives, read_check_directives};

/// `FileCheckString::CheckSame`: the match stays on the previous match's line.
#[test]
fn check_same_stays_on_the_line() {
    check_directives("a b\nc\n", &[Check::Line("a"), Check::Same("b")]);
}

/// `FileCheckString::CheckSame`'s refusal.
#[test]
#[should_panic(expected = "CHECK-SAME: is not on the same line as the previous match")]
fn check_same_refuses_the_next_line() {
    check_directives("a b\nc\n", &[Check::Line("a"), Check::Same("c")]);
}

/// `FileCheck::readCheckFile`: a leading `-SAME` has no line to stay on.
#[test]
#[should_panic(expected = "found 'CHECK-SAME' without previous 'CHECK: line")]
fn check_same_needs_a_previous_directive() {
    check_directives("a\n", &[Check::Same("a")]);
}

/// `Pattern::parsePattern` and `Pattern::match`: `[[NAME:...]]` records what it
/// matched and `[[NAME]]` asks for exactly that.
#[test]
fn a_variable_is_used_as_defined() {
    check_directives(
        "%x = add\nuse %x\n",
        &[Check::Line("[[X:%.*]] = add"), Check::Next("use [[X]]")],
    );
}

/// `[[NAME]]` asks for the captured text, not for anything the definition's
/// regex would match.
#[test]
#[should_panic(expected = "not found in:")]
fn a_variable_refuses_another_value() {
    check_directives(
        "%x = add\nuse %y\n",
        &[Check::Line("[[X:%.*]] = add"), Check::Next("use [[X]]")],
    );
}

/// `Pattern::match`: `use of undefined variable`, here for a name never defined.
#[test]
#[should_panic(expected = "undefined variable: X")]
fn an_undefined_variable_is_refused() {
    check_directives("a\n", &[Check::Line("[[X]]")]);
}

/// `Pattern::match` compiles with `Regex::Newline`, so `.*` keeps to one line.
#[test]
#[should_panic(expected = "not found in:")]
fn a_regex_block_stops_at_the_line_end() {
    check_directives("a\nb\n", &[Check::Line("a{{.*}}b")]);
}

/// `.*` takes the longest extent on the line that lets the rest match, backing
/// off from its greedy reach: `[[V:%.*]]` first takes the whole line's tail,
/// and gives it back until `, ptr` matches, so `V` is `%x`.
#[test]
fn a_regex_block_backs_off() {
    check_directives(
        "store i32 %x, ptr %y, align 4\nuse %x\n",
        &[
            Check::Line("store i32 [[V:%.*]], ptr {{.*}}, align 4"),
            Check::Next("use [[V]]"),
        ],
    );
}

/// `FileCheck::checkInput`: the text past a `CHECK-LABEL` belongs to the
/// directives after it, so a directive before the label cannot match there.
#[test]
#[should_panic(expected = "CHECK: \"after\" not found in:")]
fn a_label_bounds_the_directives_before_it() {
    check_directives(
        "label\nafter\n",
        &[Check::Line("after"), Check::Label("label")],
    );
}

/// `FileCheck::checkInput`: a label is found first, then the directives up to
/// it match in the text before it.
#[test]
fn a_label_is_found_before_the_directives_it_ends() {
    check_directives(
        "before\nlabel\nafter\n",
        &[
            Check::Line("before"),
            Check::Label("label"),
            Check::Next("after"),
        ],
    );
}

/// `FileCheck::readCheckFile` refuses a label that defines or uses a variable.
#[test]
#[should_panic(expected = "found 'CHECK-LABEL:' with variable definition or use")]
fn a_label_cannot_define_a_variable() {
    check_directives("a\n", &[Check::Label("[[X:.*]]")]);
}

/// A regex this subset does not read is a panic, not a literal: reading it as
/// text would silently check something else.
#[test]
#[should_panic(expected = "this FileCheck subset reads literal text and `.*`, not: [0-9]+")]
fn an_unimplemented_regex_is_refused() {
    check_directives("a\n", &[Check::Line("{{[0-9]+}}")]);
}

/// `Pattern::parsePattern` keeps the first `[` of a `[[[` as text.
#[test]
fn a_third_bracket_is_text() {
    check_directives(
        "[%x\n%x\n",
        &[Check::Line("[[[X:%.*]]"), Check::Next("[[X]]")],
    );
}

/// `FileCheck::readCheckFile`: each directive to its line's end, and no
/// `CHECK` inside a word, such as the `FileCheck` of a `RUN` line.
#[test]
fn directives_are_read_from_the_fixture() {
    let source = "; RUN: opt | FileCheck %s\n\
                  ; CHECK-LABEL: define @f(\n\
                  ; CHECK-SAME: i32 [[X:%.*]])\n\
                  ; CHECK-NEXT: ret i32 [[X]]\n\
                  ; CHECK: }\n\
                  define i32 @f(i32 %x) {\n";
    let checks = read_check_directives(source);
    let names: Vec<(&str, &str)> = checks
        .iter()
        .map(|check| match check {
            Check::Line(pattern) => ("line", pattern.trim()),
            Check::Next(pattern) => ("next", pattern.trim()),
            Check::Same(pattern) => ("same", pattern.trim()),
            Check::Label(pattern) => ("label", pattern.trim()),
        })
        .collect();
    assert_eq!(
        names,
        [
            ("label", "define @f("),
            ("same", "i32 [[X:%.*]])"),
            ("next", "ret i32 [[X]]"),
            ("line", "}"),
        ]
    );
}

/// A directive this subset does not implement is a panic rather than a skipped
/// line: the fixture is unported, not narrowed.
#[test]
#[should_panic(expected = "this FileCheck subset does not read the directive in")]
fn an_unimplemented_directive_is_refused() {
    read_check_directives("; CHECK-NOT: a\n");
}
