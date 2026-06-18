; Excerpted from llvm/test/Assembler/aggregate-constant-values.ll.
; RUN: llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s
; RUN: verify-uselistorder %s

; CHECK: @foo
; CHECK: store { i32, i32 } { i32 7, i32 9 }, ptr %x
; CHECK: ret
define void @foo(ptr %x) nounwind {
  store {i32, i32}{i32 7, i32 9}, ptr %x
  ret void
}
