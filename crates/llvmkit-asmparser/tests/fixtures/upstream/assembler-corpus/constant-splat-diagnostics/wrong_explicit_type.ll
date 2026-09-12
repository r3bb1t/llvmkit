; WRONG_EXPLICIT_TYPE: error: constant expression type mismatch: got type 'i8' but expected 'i32'
define <4 x i32> @wrong_explicit_type() {
  ret <4 x i32> splat (i8 7)
}

