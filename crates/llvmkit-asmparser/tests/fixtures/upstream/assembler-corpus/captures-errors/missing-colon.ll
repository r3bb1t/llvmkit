

































; CHECK-MISSING-COLON: <stdin>:[[@LINE+1]]:36: error: expected ':'
define void @test(ptr captures(ret address) %p) {
  ret void
}

