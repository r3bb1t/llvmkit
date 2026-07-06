; Excerpted from llvm/test/Assembler/flags.ll (gep_nusw_nuw).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define ptr @gep_nusw_nuw(ptr %p, i64 %idx) {
; CHECK: %gep = getelementptr nusw nuw i8, ptr %p, i64 %idx
  %gep = getelementptr nusw nuw i8, ptr %p, i64 %idx
  ret ptr %gep
}
