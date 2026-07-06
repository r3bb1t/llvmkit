; Positive guard for llvmkit's retired dedicated `undef`-callee arm:
; `undef` callees ride the same generic value path as every other
; non-global callee (llvm/lib/AsmParser/LLParser.cpp::convertValIDToValue
; `t_Undef`) and must keep parsing after the special case's removal.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define void @caller() {
entry:
; CHECK: call void undef()
  call void () undef()
  ret void
}
