; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i64 @udiv_exact(i64 %x, i64 %y) {
; CHECK: %z = udiv exact i64 %x, %y
	%z = udiv exact i64 %x, %y
	ret i64 %z
}
