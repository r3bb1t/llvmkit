; NOT_A_VECTOR: error: vector constant must have vector type
define <4 x i32> @not_a_vector() {
  ret i32 splat (i32 7)
}
