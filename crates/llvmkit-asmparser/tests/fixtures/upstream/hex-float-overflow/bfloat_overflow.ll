define bfloat @test_bfloat_overflow() {
  %1 = fadd bfloat 0xR5F2F00, 0xR0000
  ret bfloat %1
}
