; Excerpted from llvm/test/Assembler/flags.ll (gep_nusw_nuw_vec).
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

define <2 x ptr> @gep_nusw_nuw_vec(<2 x ptr> %p, i64 %idx) {
; CHECK: %gep = getelementptr nusw nuw i8, <2 x ptr> %p, i64 %idx
  %gep = getelementptr nusw nuw i8, <2 x ptr> %p, i64 %idx
  ret <2 x ptr> %gep
}
