; Source: llvm/test/Bitcode/vectorInstructions.3.2.ll

define void @shufflevector(<2 x i8> %x1){
entry:
; CHECK: %res1 = shufflevector <2 x i8> %x1, <2 x i8> %x1, <2 x i32> <i32 0, i32 1>
  %res1 = shufflevector <2 x i8> %x1, <2 x i8> %x1, <2 x i32> <i32 0, i32 1>

; CHECK-NEXT: %res2 = shufflevector <2 x i8> %x1, <2 x i8> undef, <2 x i32> <i32 0, i32 1>
  %res2 = shufflevector <2 x i8> %x1, <2 x i8> undef, <2 x i32> <i32 0, i32 1>

  ret void
}
