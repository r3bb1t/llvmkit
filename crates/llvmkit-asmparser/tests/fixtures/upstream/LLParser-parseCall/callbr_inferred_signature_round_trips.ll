; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseCallBr: with no explicit
; call-site function type, the callbr type is inferred from the argument
; list (`void (float)` here) and the callee `@f` resolves as a bare
; pointer, so the callbr carries its own function type independent of the
; declaration (the callee-kind restriction is the verifier's concern, not
; the parser's). Non-vararg, so it re-prints in AsmWriter's short
; (return-type-only) form.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(i32)

define void @g() {
entry:
; CHECK: callbr void @f(float 0.000000e+00)
  callbr void @f(float 0.0)
          to label %cont []

cont:
  ret void
}
