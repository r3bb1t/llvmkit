; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i1 @test_icmp_samesign(i32 %a, i32 %b) {
  ; CHECK: %res = icmp samesign ult i32 %a, %b
  %res = icmp samesign ult i32 %a, %b
  ret i1 %res
}
