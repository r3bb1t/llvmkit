; llvmkit-specific STRICTNESS lock, explicit-type invoke form. Upstream
; llvm-as ACCEPTS this module: with an explicit call-site type,
; llvm/lib/AsmParser/LLParser.cpp::parseInvoke takes the written
; `void (i8)` as the call-site signature, and under opaque pointers a
; direct invoke through a mismatched declaration is legal IR (runtime
; UB, not a parse error). llvmkit compares the call-site signature
; against the declaration and rejects at parse time
; (ll_parser.rs::resolve_direct_callee), same doctrine as the
; inferred-signature locks beside this fixture.

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
  invoke void (i8) @f(i8 1)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
