define bfloat @test_bfloat_overflow() {
; BFLOAT: error: hexadecimal constant too large for bfloat (16-bit)
  %1 = fadd bfloat 0xR5F2F00, 0xR0000
  ret bfloat %1
}

