; Crafted against llvm/lib/AsmParser/LLParser.cpp::convertValIDToValue's
; `t_Null` arm with a pointer target type: `null` is a legal (if
; degenerate) callee upstream; LLVM 22.1.4 ships no lit coverage of the
; spelling, so the rule shape is the anchor (D11).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define void @caller() {
entry:
; CHECK: call void null()
  call void () null()
  ret void
}
