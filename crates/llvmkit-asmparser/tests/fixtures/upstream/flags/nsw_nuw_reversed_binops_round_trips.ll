; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i64 @add_both_reversed(i64 %x, i64 %y) {
; CHECK: %z = add nuw nsw i64 %x, %y
	%z = add nsw nuw i64 %x, %y
	ret i64 %z
}

define i64 @sub_both_reversed(i64 %x, i64 %y) {
; CHECK: %z = sub nuw nsw i64 %x, %y
	%z = sub nsw nuw i64 %x, %y
	ret i64 %z
}

define i64 @mul_both_reversed(i64 %x, i64 %y) {
; CHECK: %z = mul nuw nsw i64 %x, %y
	%z = mul nsw nuw i64 %x, %y
	ret i64 %z
}
