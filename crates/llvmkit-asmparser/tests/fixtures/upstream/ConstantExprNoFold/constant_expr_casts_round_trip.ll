; Excerpted from llvm/test/Assembler/ConstantExprNoFold.ll.
; This test checks to make sure that constant exprs don't fold in some simple
; situations
;
; RUN: llvm-as < %s | llvm-dis | FileCheck %s
; RUN: verify-uselistorder %s

; Even give it a datalayout, to tempt folding as much as possible.
target datalayout = "p:32:32"

@A = global i64 0

; CHECK: @E = global ptr addrspace(1) addrspacecast (ptr @A to ptr addrspace(1))
@E = global ptr addrspace(1) addrspacecast(ptr @A to ptr addrspace(1))
