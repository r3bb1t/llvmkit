; Crafted against llvm/lib/AsmParser/LLParser.cpp::parseInvoke's argument
; loop ("argument is not of expected type") with an explicit call-site
; type; no upstream lit or unittest coverage of the diagnostic at 22.1.4,
; rule shape is the anchor (D11). llvmkit routes the check through
; `validate_call_site_args` in `build_invoke_dyn_with_config`.

declare void @f(i32)
declare i32 @__gxx_personality_v0(...)

define void @g() personality ptr @__gxx_personality_v0 {
entry:
  invoke void (i32) @f(float 0.0)
          to label %ok unwind label %lp

ok:
  ret void

lp:
  %pad = landingpad { ptr, i32 }
          cleanup
  ret void
}
