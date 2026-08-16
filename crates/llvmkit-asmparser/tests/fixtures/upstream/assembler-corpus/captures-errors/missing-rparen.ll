


















; CHECK-MISSING-RPAREN: <stdin>:[[@LINE+1]]:40: error: expected ',' or ')'
define void @test(ptr captures(address %p) {
  ret void
}
