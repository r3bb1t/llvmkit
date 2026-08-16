; CHECK-INCORRECT-STRUCT-ELEMENTS: number of output constraints does not match number of return struct elements
define void @foo() {
  call { i32 } asm sideeffect "mov x0, #42", "=r,=r"()
  ret void
}
