; Crafted against the rejection rule of
; llvm/lib/AsmParser/LLParser.cpp::parseCall's post-loop parameter check.
; LLVM 22.1.4 ships no lit or unittest coverage for this diagnostic, so
; the rule itself is the anchor (D11).
; RUN: not llvm-as < %s 2>&1 | FileCheck %s

declare void @f(i32, i32)

define void @g() {
entry:
; CHECK: error: not enough parameters specified for call
  call void (i32, i32) @f(i32 1)
  ret void
}
