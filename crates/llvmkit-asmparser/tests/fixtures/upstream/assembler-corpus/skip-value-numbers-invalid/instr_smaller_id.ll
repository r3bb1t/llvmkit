
; INSTR-SMALLER-ID: error: instruction expected to be numbered '%11' or greater
define i32 @test() {
  %10 = add i32 1, 2
  %5 = add i32 %10, 3
  ret i32 %5
}

