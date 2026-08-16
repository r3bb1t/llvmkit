; CHECK-CANNOT-BE-STRUCT: inline asm with one output cannot return struct
define void @foo() {
  call { i32 } asm sideeffect "mov x0, #42", "=r"()
  ret void
}
