define half @test_half_overflow() {
  %1 = fadd half 0xH5F2F00, 0xH0000
  ret half %1
}
