; NAME-FOLLOWS-INT: error: expected ')'
define void @nofpclass_name_plus_int(float nofpclass(42 nan) %x) {
  ret void
}
