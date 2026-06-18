; Copied from llvm/test/Assembler/flags.ll::const_gep_nusw_nuw_as1.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

@addr_as1 = external addrspace(1) global i64

define ptr addrspace(1) @const_gep_nusw_nuw_as1() {
; CHECK: ret ptr addrspace(1) getelementptr nusw nuw (i8, ptr addrspace(1) @addr_as1, i64 100)
  ret ptr addrspace(1) getelementptr nusw nuw (i8, ptr addrspace(1) @addr_as1, i64 100)
}
