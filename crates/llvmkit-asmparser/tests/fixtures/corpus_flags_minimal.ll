; llvmkit corpus fixture derived from upstream `test/Assembler/flags.ll`.
define i64 @add_both(i64 %x, i64 %y) {
entry:
  %z = add nuw nsw i64 %x, %y
  ret i64 %z
}
