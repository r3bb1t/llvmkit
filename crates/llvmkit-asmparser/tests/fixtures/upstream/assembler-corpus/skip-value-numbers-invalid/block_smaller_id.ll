
; BLOCK-SMALLER-ID: error: label expected to be numbered '11' or greater
define i32 @test() {
10:
  br label %5

5:
  ret i32 0
}
