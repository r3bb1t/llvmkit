
; ARG-SMALLER-ID: error: argument expected to be numbered '%11' or greater
define i32 @test(i32 %10, i32 %5) {
  ret i32 %5
}

