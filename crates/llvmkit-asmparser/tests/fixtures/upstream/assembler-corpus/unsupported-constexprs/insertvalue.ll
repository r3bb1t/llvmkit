define {i32} @insertvalue() {
; INSERTVALUE: error: insertvalue constexprs are no longer supported
  ret {i32} insertvalue ({i32} poison, i32 3, 0)
}
