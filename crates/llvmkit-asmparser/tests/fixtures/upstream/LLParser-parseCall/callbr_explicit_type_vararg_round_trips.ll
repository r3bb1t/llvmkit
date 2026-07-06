; Crafted against llvm/lib/AsmParser/LLParser.cpp::resolveFunctionType's
; FunctionType branch reached from parseCallBr, vararg form: like invoke,
; a vararg callbr call site is only expressible through the explicit
; type, and AsmWriter keeps the long-form spelling. Parse-level mirror
; only: upstream's verifier additionally requires a direct intrinsic
; callee for non-asm callbr (Verifier.cpp::visitCallBrInst), so this
; module is llvm-as-clean but not opt-verify-clean upstream.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @g(i32, ...)

define void @caller() {
entry:
; CHECK: callbr void (i32, ...) @g(i32 1, i8 2)
  callbr void (i32, ...) @g(i32 1, i8 2)
          to label %cont []

cont:
  ret void
}
