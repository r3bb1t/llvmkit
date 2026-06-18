; Excerpted from llvm/test/Assembler/aggregate-constant-values.ll.
; RUN: llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s
; RUN: verify-uselistorder %s

; CHECK: @bar
; CHECK: store [2 x i32] [i32 7, i32 9], ptr %x
; CHECK: ret
define void @bar(ptr %x) nounwind {
  store [2 x i32][i32 7, i32 9], ptr %x
  ret void
}
