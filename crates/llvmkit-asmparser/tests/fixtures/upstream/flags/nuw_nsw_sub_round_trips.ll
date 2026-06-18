; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i64 @sub_both(i64 %x, i64 %y) {
; CHECK: %z = sub nuw nsw i64 %x, %y
	%z = sub nuw nsw i64 %x, %y
	ret i64 %z
}
