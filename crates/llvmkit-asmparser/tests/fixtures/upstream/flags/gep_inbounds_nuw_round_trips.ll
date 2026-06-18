; Excerpted from llvm/test/Assembler/flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define ptr @gep_inbounds_nuw(ptr %p, i64 %idx) {
; CHECK: %gep = getelementptr inbounds nuw i8, ptr %p, i64 %idx
  %gep = getelementptr inbounds nuw i8, ptr %p, i64 %idx
  ret ptr %gep
}
