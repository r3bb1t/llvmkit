; Crafted against llvm/lib/AsmParser/LLParser.cpp::parseCallBr's argument
; loop ("argument is not of expected type") with an explicit call-site
; type; no upstream lit or unittest coverage of the diagnostic at 22.1.4,
; rule shape is the anchor (D11). llvmkit routes the check through
; `validate_call_site_args` in `build_callbr_with_config`.

declare void @g(i32)

define void @caller() {
entry:
  callbr void (i32) @g(float 0.0)
          to label %cont []

cont:
  ret void
}
