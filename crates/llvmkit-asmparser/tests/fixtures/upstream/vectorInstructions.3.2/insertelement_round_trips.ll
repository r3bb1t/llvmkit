; Source: llvm/test/Bitcode/vectorInstructions.3.2.ll

define void @insertelement(<2 x i8> %x1){
entry:
; CHECK: %res1 = insertelement <2 x i8> %x1, i8 0, i32 0
  %res1 = insertelement <2 x i8> %x1, i8 0, i32 0

  ret void
}
