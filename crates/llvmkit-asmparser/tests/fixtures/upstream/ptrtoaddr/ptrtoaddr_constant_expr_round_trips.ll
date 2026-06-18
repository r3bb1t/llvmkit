; Excerpted from llvm/test/Assembler/ptrtoaddr.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

@i_as0 = global i32 0
@global_cast_as0 = global i64 ptrtoaddr (ptr @i_as0 to i64)
; CHECK: @global_cast_as0 = global i64 ptrtoaddr (ptr @i_as0 to i64)
