; Crafted against llvm/lib/AsmParser/LLParser.cpp::parseCallBr, which shares
; parseCall's callee path and stores whatever `Value *` it resolves to, so an
; indirect callbr PARSES; llvm/lib/IR/Verifier.cpp::visitCallBrInst is what
; rejects it, with "Callbr: indirect function / invalid signature".
; LLVM 22.1.4 ships no lit fixture for the parse half on its own.

define void @caller(ptr %fp) {
entry:
  callbr void (i32) %fp(i32 1)
          to label %cont []

cont:
  ret void
}
