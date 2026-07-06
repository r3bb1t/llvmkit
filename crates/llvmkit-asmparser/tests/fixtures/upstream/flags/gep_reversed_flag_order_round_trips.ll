; Excerpted from llvm/test/Assembler/flags.ll (gep_nuw_nusw_inbounds):
; flags in NON-canonical order parse and re-print canonically
; (inbounds implies nusw, so nusw is suppressed in the output).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define ptr @gep_nuw_nusw_inbounds(ptr %p, i64 %idx) {
; CHECK: %gep = getelementptr inbounds nuw i8, ptr %p, i64 %idx
  %gep = getelementptr nuw nusw inbounds i8, ptr %p, i64 %idx
  ret ptr %gep
}
