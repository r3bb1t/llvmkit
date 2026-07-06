; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseInvoke's use of
; resolveFunctionType: when the written type is already a FunctionType,
; it IS the call-site type (no inference from the argument list). The
; non-vararg invoke prints back in AsmWriter's short form.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke void @f(i32 1)
  invoke void (i32) @f(i32 1)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
