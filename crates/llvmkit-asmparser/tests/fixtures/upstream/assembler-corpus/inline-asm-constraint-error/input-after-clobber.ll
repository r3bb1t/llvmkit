; CHECK-INPUT-AFTER-CLOBBER: input constraint occurs after clobber constraint
define void @foo() {
  call void asm sideeffect "mov x0, #42", "~{x0},r"()
  ret void
}
