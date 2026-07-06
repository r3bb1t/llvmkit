; Crafted against the rejection rule of
; llvm/lib/AsmParser/LLParser.cpp::parseCall's argument loop (the
; non-vararg overflow arm). LLVM 22.1.4 ships no lit or unittest coverage
; for this diagnostic, so the rule itself is the anchor (D11).
; RUN: not llvm-as < %s 2>&1 | FileCheck %s

declare void @f()

define void @g() {
entry:
; CHECK: error: too many arguments specified
  call void () @f(i32 1)
  ret void
}
