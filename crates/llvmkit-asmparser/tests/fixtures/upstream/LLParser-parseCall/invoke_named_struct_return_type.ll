; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseBasicBlock + parseInvoke:
; parseBasicBlock strips the optional `%name =` before parseInstruction
; dispatches, so the token parseInvoke reads next is parseType's return type.
; A `%`-sigil token there is a named struct type and nothing else -- there is
; no result name left to confuse it with, in either the named or the bare
; spelling.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

%struct.S = type { i32, i32 }

declare %struct.S @f()
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
; CHECK: invoke %struct.S @f()
  invoke %struct.S @f()
          to label %ok unwind label %lp

ok:
; CHECK: %r = invoke %struct.S @f()
  %r = invoke %struct.S @f()
          to label %done unwind label %lp

done:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
