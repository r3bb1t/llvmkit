; llvmkit-authored: upstream's own splits of
; llvm/test/Assembler/inline-asm-constraint-error.ll all stop at
; InlineAsm::verify, so none of them reaches Verifier::verifyInlineAsmCall's
; "Label constraints can only be used with callbr". This one does.

define void @foo() {
  call void asm sideeffect "", "!i"()
  ret void
}
