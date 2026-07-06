; Crafted against the rejection rule of
; llvm/lib/AsmParser/LLParser.cpp::parseCall's argument loop: the check is
; type IDENTITY, so an i8 argument against an i32 parameter is rejected
; even though both are integers. LLVM 22.1.4 ships no lit or unittest
; coverage for this diagnostic, so the rule itself is the anchor (D11).
; RUN: not llvm-as < %s 2>&1 | FileCheck %s

declare void @f(i32)

define void @g() {
entry:
; CHECK: error: argument is not of expected type 'i32'
  call void (i32) @f(i8 1)
  ret void
}
