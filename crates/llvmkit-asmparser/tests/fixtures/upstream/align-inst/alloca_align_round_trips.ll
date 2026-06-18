; Excerpted from llvm/test/Assembler/align-inst.ll.
; RUN: llvm-as %s -o /dev/null
; RUN: verify-uselistorder %s

define void @foo() {
  %p = alloca i1, align 4294967296
  ret void
}
