; NOT_A_CONSTANT: error: expected instruction opcode
define <4 x i32> @not_a_constant(i32 %a) {
  %splat = splat (i32 %a)
  ret <vscale x 4 x i32> %splat
}
