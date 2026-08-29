; OUTER-COMMA: expected ')'
define void @foo(ptr initializes((0, 4) (8, 12)) %a) {
  ret void
}

