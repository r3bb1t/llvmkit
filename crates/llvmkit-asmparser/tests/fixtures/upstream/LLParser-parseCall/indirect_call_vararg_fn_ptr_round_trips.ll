; Mirrors llvm/test/Assembler/call-arg-is-callee.ll's `@call`: an explicit
; vararg call-site type through a local function pointer exercises
; llvm/lib/AsmParser/LLParser.cpp::resolveFunctionType's FunctionType
; branch together with the indirect-callee path. Vararg call sites keep
; AsmWriter's long-form type spelling.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define void @caller(ptr %fp) {
entry:
; CHECK: call void (i32, ...) %fp(i32 1, i8 2)
  call void (i32, ...) %fp(i32 1, i8 2)
  ret void
}
