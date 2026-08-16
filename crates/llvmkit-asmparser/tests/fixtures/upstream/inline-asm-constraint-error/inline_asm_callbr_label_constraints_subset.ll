; llvmkit-authored: the callbr twin of the file beside it, reaching
; Verifier::verifyInlineAsmCall's "Number of label constraints does not match
; number of callbr dests" — one indirect destination, no label constraint.

define void @foo() {
  callbr void asm sideeffect "", ""()
  to label %1 [label %2]
1:
  ret void
2:
  ret void
}
