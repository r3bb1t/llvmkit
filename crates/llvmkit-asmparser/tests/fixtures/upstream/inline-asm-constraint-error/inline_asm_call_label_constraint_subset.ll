; llvmkit-specific subset of llvm/test/Assembler/inline-asm-constraint-error.ll.
; Upstream validates the full inline-asm constraint grammar. llvmkit currently
; models the parser-level rule that label constraints are only legal for callbr.

define void @foo() {
  call void asm sideeffect "", "!i"()
  ret void
}
