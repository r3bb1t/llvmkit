; CHECK-MUST-RETURN-VOID: inline asm without outputs must return void
define void @foo() {
  call i32 asm sideeffect "mov x0, #42", ""()
  ret void
}

