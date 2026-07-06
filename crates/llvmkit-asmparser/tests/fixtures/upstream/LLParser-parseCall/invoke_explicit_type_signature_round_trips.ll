; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseInvoke's resolveFunctionType:
; the explicitly-written call-site type `void (i8)` IS the invoke's function
; type, independent of `@f`'s `void (i32)` declaration. The callee resolves
; as a bare pointer, so this is legal IR under opaque pointers; the verifier,
; not the parser, owns the call-vs-declaration check. Non-vararg, so the
; explicit function type is dropped when re-printed (AsmWriter short form).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke void @f(i8 1)
  invoke void (i8) @f(i8 1)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
