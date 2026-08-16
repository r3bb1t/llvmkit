







































; CHECK-INVALID-COMPONENT: <stdin>:[[@LINE+1]]:32: error: expected one of 'none', 'address', 'address_is_null', 'provenance' or 'read_provenance'
define void @test(ptr captures(foo) %p) {
  ret void
}
