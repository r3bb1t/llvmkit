; Mirrors llvm/test/Feature/indirectcall.ll's `call i64 %fibfunc(...)`: a
; callee may be any pointer-typed value, parsed through
; llvm/lib/AsmParser/LLParser.cpp::parseCall's `parseValID` +
; `convertValIDToValue(PointerType)` path rather than a dedicated
; function-name arm. Non-vararg indirect calls print in AsmWriter's short
; form (result type only).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define void @caller(ptr %fp) {
entry:
; CHECK: call void %fp(i32 1)
  call void (i32) %fp(i32 1)
  ret void
}
