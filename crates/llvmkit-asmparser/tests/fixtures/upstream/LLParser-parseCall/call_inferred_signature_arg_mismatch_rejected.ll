; llvmkit-specific STRICTNESS lock. Upstream llvm-as ACCEPTS this module:
; with no explicit call-site function type,
; llvm/lib/AsmParser/LLParser.cpp::parseCall infers `void (i32)` from the
; argument list, and under opaque pointers a direct call through a
; mismatched signature is legal IR (runtime UB, not a parse error).
; llvmkit instead compares the inferred call-site signature against the
; declaration and rejects at parse time
; (ll_parser.rs::resolve_direct_callee).

declare void @f()

define void @g() {
entry:
  call void @f(i32 1)
  ret void
}
