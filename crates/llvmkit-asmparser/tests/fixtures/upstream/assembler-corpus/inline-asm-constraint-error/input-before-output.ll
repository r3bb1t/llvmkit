; CHECK-INPUT-BEFORE-OUTPUT: output constraint occurs after input, clobber or label constraint
define void @foo() {
  call void asm sideeffect "mov x0, #42", "r,=r"()
  ret void
}

