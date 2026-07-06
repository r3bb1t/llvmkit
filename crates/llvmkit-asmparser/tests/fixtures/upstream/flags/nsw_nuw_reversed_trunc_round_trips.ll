; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

; The upstream file also locks a `<2 x i64>` vector form (@test_trunc_both_reversed_vector);
; llvmkit's parse_int_cast does not support vector integer casts yet, so only the
; scalar excerpt is mirrored here.

define i32 @test_trunc_both_reversed(i64 %a) {
; CHECK: %res = trunc nuw nsw i64 %a to i32
  %res = trunc nsw nuw i64 %a to i32
  ret i32 %res
}
