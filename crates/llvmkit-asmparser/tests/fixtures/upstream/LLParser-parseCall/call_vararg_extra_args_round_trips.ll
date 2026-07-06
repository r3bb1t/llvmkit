; Positive guard for llvm/lib/AsmParser/LLParser.cpp::parseCall's vararg
; arm: arguments past the fixed parameters of a vararg callee are legal
; and must keep parsing (the negative fixtures next to this one must not
; over-reject). Crafted; the printed form matches AsmWriter's explicit
; vararg call-site type.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(i32, ...)

define void @g() {
entry:
; CHECK: call void (i32, ...) @f(i32 1, i8 2, i32 3)
  call void (i32, ...) @f(i32 1, i8 2, i32 3)
  ret void
}
