; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define i64 @test_or(i64 %a, i64 %b) {
; CHECK: %res = or disjoint i64 %a, %b
  %res = or disjoint i64 %a, %b
  ret i64 %res
}
