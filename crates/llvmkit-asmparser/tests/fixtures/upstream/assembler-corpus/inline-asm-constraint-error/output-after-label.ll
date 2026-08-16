; CHECK-OUTPUT-AFTER-LABEL: output constraint occurs after input, clobber or label constraint
define void @foo() {
  %res = callbr i32 asm sideeffect "", "!i,=r,~{flags}"()
  to label %1 [label %2]
1:
  ret void
2:
  ret void
}
