; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseInvoke: with no explicit
; call-site function type, the invoke type is inferred from the argument
; list (`void (float)` here) and the callee `@f` resolves as a bare
; pointer, so the invoke carries its own function type independent of the
; declaration. Under opaque pointers this is legal IR; the verifier, not
; the parser, owns the call-vs-declaration check. Non-vararg, so it
; re-prints in AsmWriter's short (return-type-only) form.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke void @f(float 0.000000e+00)
  invoke void @f(float 0.0)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
