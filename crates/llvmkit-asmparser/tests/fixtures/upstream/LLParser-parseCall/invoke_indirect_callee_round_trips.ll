; Mirrors llvm/test/Assembler/call-arg-is-callee.ll's `@invoke`: an invoke
; through a local function pointer. llvm/lib/AsmParser/LLParser.cpp::parseInvoke
; resolves the callee via `convertValIDToValue(PointerType)` exactly like
; parseCall, and the indirect invoke is valid IR (Verifier::visitCallBase
; accepts an indirect callee). The varargs call-site type prints in full.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare i32 @__gxx_personality_v0(...)

define void @caller(ptr %p) personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke void (...) %p(ptr %p)
  invoke void (...) %p(ptr %p)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
