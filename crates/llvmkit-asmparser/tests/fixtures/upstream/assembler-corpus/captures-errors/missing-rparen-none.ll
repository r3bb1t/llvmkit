

























; CHECK-MISSING-RPAREN-NONE: <stdin>:[[@LINE+1]]:37: error: expected ',' or ')'
define void @test(ptr captures(none %p) {
  ret void
}
