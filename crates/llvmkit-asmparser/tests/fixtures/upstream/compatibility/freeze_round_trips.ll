; Source: llvm/test/Bitcode/compatibility.ll
; llvmkit-specific subset: parser requires a result name for value-producing
; instructions, so this fixture names the freeze result.

define i32 @freeze(i32 %op1) {
  %res = freeze i32 %op1
  ; CHECK: freeze i32 %op1
  ret i32 %res
}
