; Source: llvm/test/Bitcode/vectorInstructions.3.2.ll

define void @extractelement(<2 x i8> %x1){
entry:
; CHECK: %res1 = extractelement <2 x i8> %x1, i32 0
  %res1 = extractelement <2 x i8> %x1, i32 0

  ret void
}
