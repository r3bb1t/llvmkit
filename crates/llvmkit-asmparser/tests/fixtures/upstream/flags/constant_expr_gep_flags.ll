; Excerpted from llvm/test/Assembler/flags.ll constant-expression GEP flag cases.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s
; RUN: verify-uselistorder %s

@addr = external global i64
@addr_as1 = external addrspace(1) global i64

define ptr @const_gep_nuw() {
; CHECK: ret ptr getelementptr nuw (i8, ptr @addr, i64 100)
  ret ptr getelementptr nuw (i8, ptr @addr, i64 100)
}

define ptr @const_gep_inbounds_nuw() {
; CHECK: ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)
  ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)
}

define ptr @const_gep_nusw() {
; CHECK: ret ptr getelementptr nusw (i8, ptr @addr, i64 100)
  ret ptr getelementptr nusw (i8, ptr @addr, i64 100)
}

; inbounds implies nusw, so the flag is not printed back.
define ptr @const_gep_inbounds_nusw() {
; CHECK: ret ptr getelementptr inbounds (i8, ptr @addr, i64 100)
  ret ptr getelementptr inbounds nusw (i8, ptr @addr, i64 100)
}

define ptr @const_gep_nusw_nuw() {
; CHECK: ret ptr getelementptr nusw nuw (i8, ptr @addr, i64 100)
  ret ptr getelementptr nusw nuw (i8, ptr @addr, i64 100)
}

define ptr @const_gep_inbounds_nusw_nuw() {
; CHECK: ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)
  ret ptr getelementptr inbounds nusw nuw (i8, ptr @addr, i64 100)
}

define ptr @const_gep_nuw_nusw_inbounds() {
; CHECK: ret ptr getelementptr inbounds nuw (i8, ptr @addr, i64 100)
  ret ptr getelementptr nuw nusw inbounds (i8, ptr @addr, i64 100)
}

define ptr @const_gep_nuw_inrange() {
; CHECK: ret ptr getelementptr nuw inrange(-8, 16) (i8, ptr @addr, i64 100)
  ret ptr getelementptr nuw inrange(-8, 16) (i8, ptr @addr, i64 100)
}
