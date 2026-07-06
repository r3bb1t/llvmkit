; llvmkit-specific STRICTNESS lock, callbr form. Upstream llvm-as ACCEPTS
; this module at parse time: llvm/lib/AsmParser/LLParser.cpp::parseCallBr
; infers `void (float)` from the argument list and builds the callbr with
; that type (the callee-kind restriction is the verifier's concern, not
; the parser's). llvmkit instead compares the inferred call-site
; signature against the declaration and rejects at parse time
; (ll_parser.rs::resolve_direct_callee).

declare void @f(i32)

define void @g() {
entry:
  callbr void @f(float 0.0)
          to label %cont []

cont:
  ret void
}
