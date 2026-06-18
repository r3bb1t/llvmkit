; llvmkit-specific subset of llvm/test/Assembler/inline-asm-constraint-error.ll.
; Upstream checks full inline-asm constraint ordering diagnostics; llvmkit
; currently models the callbr label-constraint count invariant.

define void @foo() {
  callbr void asm sideeffect "", ""()
  to label %1 [label %2]
1:
  ret void
2:
  ret void
}
