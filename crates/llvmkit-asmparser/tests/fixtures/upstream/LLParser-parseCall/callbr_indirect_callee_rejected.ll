; llvmkit-specific STRICTNESS lock. Upstream PARSES an indirect callbr
; (llvm/lib/AsmParser/LLParser.cpp::parseCallBr shares parseCall's callee
; path) but rejects it in the verifier:
; llvm/lib/IR/Verifier.cpp::visitCallBrInst requires a direct callee for
; non-asm callbr ("Callbr: indirect function / invalid signature").
; llvmkit rejects at parse time instead.

define void @caller(ptr %fp) {
entry:
  callbr void (i32) %fp(i32 1)
          to label %cont []

cont:
  ret void
}
