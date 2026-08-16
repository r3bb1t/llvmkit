define i32 @instr() {
  %10 = add i32 1, 2
  %20 = add i32 %10, 3
  %30 = add i32 %20, 4
  ret i32 %30
}
