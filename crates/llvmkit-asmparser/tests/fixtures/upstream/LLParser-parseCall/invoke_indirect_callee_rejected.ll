; llvmkit-specific GAP lock. Upstream llvm-as ACCEPTS this module —
; llvm/test/Assembler/call-arg-is-callee.ll's `@invoke` invokes through a
; local function pointer, and llvm/lib/AsmParser/LLParser.cpp::parseInvoke
; resolves the callee via `convertValIDToValue(PointerType)` exactly like
; parseCall. llvmkit has no indirect-invoke builder yet, so the parser
; rejects the resolved indirect callee with a deliberate diagnostic
; instead of the pre-port generic parse failure.

declare i32 @__gxx_personality_v0(...)

define void @caller(ptr %p) personality ptr @__gxx_personality_v0 {
entry:
  invoke void (...) %p(ptr %p)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
