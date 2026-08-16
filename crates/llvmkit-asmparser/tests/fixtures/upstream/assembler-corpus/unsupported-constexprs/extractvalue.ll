define i32 @extractvalue() {
; EXTRACTVALUE: error: extractvalue constexprs are no longer supported
  ret i32 extractvalue ({i32} {i32 3}, 0)
}
