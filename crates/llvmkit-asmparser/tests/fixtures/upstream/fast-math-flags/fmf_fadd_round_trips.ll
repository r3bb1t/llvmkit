; Excerpted from llvm/test/Assembler/fast-math-flags.ll.
; RUN: llvm-as < %s | llvm-dis | FileCheck -strict-whitespace %s

define float @no_nan_inf(float %x, float %y) {
entry:
; CHECK:  %a = fadd nnan ninf float %x, %y
  %a = fadd ninf nnan float %x, %y
  ret float %a
}
