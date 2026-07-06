; llvmkit-specific STRICTNESS lock, invoke form. Upstream llvm-as ACCEPTS
; this module: llvm/lib/AsmParser/LLParser.cpp::parseInvoke infers
; `void (float)` from the argument list, and under opaque pointers a
; direct invoke through a mismatched signature is legal IR (runtime UB,
; not a parse error). llvmkit instead compares the inferred call-site
; signature against the declaration and rejects at parse time
; (ll_parser.rs::resolve_direct_callee).

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
  invoke void @f(float 0.0)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
