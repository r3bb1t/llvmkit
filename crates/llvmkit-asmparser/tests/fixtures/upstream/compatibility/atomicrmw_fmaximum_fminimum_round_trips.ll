; Source: llvm/test/Bitcode/compatibility.ll, @fp_atomics (lines 918-941).
; llvmkit-specific subset: only the fmaximum/fminimum atomicrmw lines,
; the LLVM 21 IEEE-754 maximum/minimum-semantics ops added to
; AtomicRMWInst::BinOp.

define void @fp_atomics(ptr %word) {
  %atomicrmw.fmaximum = atomicrmw fmaximum ptr %word, float 1.0 monotonic
  ; CHECK: %atomicrmw.fmaximum = atomicrmw fmaximum ptr %word, float 1.000000e+00 monotonic

  %atomicrmw.fminimum = atomicrmw fminimum ptr %word, float 1.0 monotonic
  ; CHECK: %atomicrmw.fminimum = atomicrmw fminimum ptr %word, float 1.000000e+00 monotonic

  ret void
}
