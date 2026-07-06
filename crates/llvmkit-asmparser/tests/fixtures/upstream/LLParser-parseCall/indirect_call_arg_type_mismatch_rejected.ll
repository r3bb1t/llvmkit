; Crafted against the rejection rule of
; llvm/lib/AsmParser/LLParser.cpp::parseCall's argument loop, reached
; through an indirect (undef) callee so the check runs against the
; explicit call-site function type alone. LLVM 22.1.4 ships no lit or
; unittest coverage for this diagnostic, so the rule itself is the
; anchor (D11).
; RUN: not llvm-as < %s 2>&1 | FileCheck %s

define void @g() {
entry:
; CHECK: error: argument is not of expected type 'i32'
  call void (i32) undef(float 0.0)
  ret void
}
