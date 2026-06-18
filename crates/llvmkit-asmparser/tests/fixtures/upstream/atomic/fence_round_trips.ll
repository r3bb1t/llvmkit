; Source: llvm/test/Assembler/atomic.ll

define void @f(ptr %x) {
  ; CHECK: fence syncscope("singlethread") release
  fence syncscope("singlethread") release
  ; CHECK: fence seq_cst
  fence seq_cst
  ; CHECK: fence syncscope("device") seq_cst
  fence syncscope("device") seq_cst
  ret void
}
