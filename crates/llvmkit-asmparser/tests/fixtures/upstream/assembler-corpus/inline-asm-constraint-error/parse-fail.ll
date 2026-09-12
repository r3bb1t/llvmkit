; CHECK-PARSE-FAIL: failed to parse constraints
define void @foo() {
  ; "~x{21}" is not a valid clobber constraint.
  call void asm sideeffect "mov x0, #42", "~{x0},~{x19},~x{21}"()
  ret void
}

