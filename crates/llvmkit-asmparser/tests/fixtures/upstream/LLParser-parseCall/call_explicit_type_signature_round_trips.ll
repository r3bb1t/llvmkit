; Mirrors llvm/lib/AsmParser/LLParser.cpp::parseCall: the explicitly-written
; call-site type `i32 (i32)` IS the call's function type, independent of
; `@f`'s `void (float)` declaration. The callee resolves as a bare pointer,
; so this is legal IR under opaque pointers (`CallBase` carries its own
; FunctionType); the verifier, not the parser, owns the call-vs-declaration
; check. Non-vararg, so the explicit function type is dropped when
; re-printed (AsmWriter short form).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

declare void @f(float)

define i32 @g() {
entry:
; CHECK: %r = call i32 @f(i32 1)
  %r = call i32 (i32) @f(i32 1)
  ret i32 %r
}
