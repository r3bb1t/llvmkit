; Excerpted from llvm/test/Assembler/ptrauth-const.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

@var = global i32 0

; CHECK: @basic = global ptr ptrauth (ptr @var, i32 0)
@basic = global ptr ptrauth (ptr @var, i32 0)
