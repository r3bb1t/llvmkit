












; CHECK-MISSING-LPAREN: <stdin>:[[@LINE+1]]:32: error: expected '('
define void @test(ptr captures %p) {
  ret void
}

