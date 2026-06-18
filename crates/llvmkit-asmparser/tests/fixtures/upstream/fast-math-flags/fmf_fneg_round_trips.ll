; Excerpted from llvm/test/Assembler/fast-math-flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck -strict-whitespace %s

define float @no_nan(float %x, float %y) {
entry:
; CHECK:  %f = fneg nnan float %x
  %f = fneg nnan float %x
  ret float %f
}
