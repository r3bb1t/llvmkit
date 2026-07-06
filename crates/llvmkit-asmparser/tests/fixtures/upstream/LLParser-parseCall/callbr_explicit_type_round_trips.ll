; Crafted against llvm/lib/AsmParser/LLParser.cpp::resolveFunctionType's
; FunctionType branch reached from parseCallBr; LLVM 22.1.4 ships no lit
; coverage of the explicit spelling on callbr, so the rule shape is the
; anchor (D11). Non-vararg callbr prints back in short form.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @g(i32)

define void @caller() {
entry:
; CHECK: callbr void @g(i32 1)
  callbr void (i32) @g(i32 1)
          to label %cont []

cont:
  ret void
}
