; WRONG_IMPLICIT_TYPE: error: constant expression type mismatch: got type 'i8' but expected 'i32'
define void @wrong_implicit_type(<4 x i32> %a) {
  %add = add <4 x i32> %a, splat (i8 7)
  ret void
}

