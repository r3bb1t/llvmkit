; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseCall: with no explicit
; call-site function type, the call type is inferred from the argument
; list (`void (i32)` here) and the callee `@f` is resolved as a bare
; pointer, so the call carries its own function type independent of the
; declaration (a `CallBase` FunctionType). Under opaque pointers this is
; legal IR; the call-vs-declaration check belongs to the verifier, not
; the parser. The call re-prints in AsmWriter's short (return-type-only)
; form because the call-site type is not vararg.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f()

define void @g() {
entry:
; CHECK: call void @f(i32 1)
  call void @f(i32 1)
  ret void
}
