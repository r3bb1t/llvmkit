; NOT_A_SCALAR: error: constant expression type mismatch: got type '<1 x i32>' but expected 'i32'
define <4 x i32> @not_a_scalar() {
  ret <4 x i32> splat (<1 x i32> <i32 7>)
}
