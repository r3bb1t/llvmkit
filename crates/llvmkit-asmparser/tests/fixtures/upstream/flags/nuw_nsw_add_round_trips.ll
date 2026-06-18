; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i64 @add_both(i64 %x, i64 %y) {
; CHECK: %z = add nuw nsw i64 %x, %y
	%z = add nuw nsw i64 %x, %y
	ret i64 %z
}
