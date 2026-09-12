; CHECK-LABEL-AFTER-CLOBBER: label constraint occurs after clobber constraint
define void @foo() {
  callbr void asm sideeffect "", "~{flags},!i"()
  to label %1 [label %2]
1:
  ret void
2:
  ret void
}

