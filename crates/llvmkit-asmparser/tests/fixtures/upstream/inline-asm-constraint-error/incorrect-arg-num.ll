; CHECK-INCORRECT-ARG-NUM: number of input constraints does not match number of parameters
define void @foo() {
  call void asm sideeffect "mov x0, #42", "r"()
  ret void
}

