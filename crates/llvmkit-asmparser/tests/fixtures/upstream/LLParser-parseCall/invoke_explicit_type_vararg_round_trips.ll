; Crafted against llvm/lib/AsmParser/LLParser.cpp::resolveFunctionType's
; FunctionType branch reached from parseInvoke: a vararg invoke is only
; expressible through the explicit call-site type (inference would build
; a non-vararg signature that never matches the declaration). Upstream
; shape: the vararg statepoint invoke in
; llvm/test/Assembler/opaque-ptr-intrinsic-remangling.ll. Vararg call
; sites keep AsmWriter's long-form type spelling.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @vf(ptr, ...)
declare i32 @__gxx_personality_v0(...)

define void @g(ptr %p) personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke void (ptr, ...) @vf(ptr %p, i32 7)
  invoke void (ptr, ...) @vf(ptr %p, i32 7)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
