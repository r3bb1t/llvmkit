; Source: llvm/test/Bitcode/miscInstructions.3.2.ll
; llvmkit-specific subset: keeps the direct call case and drops the 3.2
; typed-pointer varargs call that opaque-pointer mode rejects.

declare i32 @test(i32)

define void @call(i32 %x) {
entry:
; CHECK: %res1 = call i32 @test(i32 %x)
  %res1 = call i32 @test(i32 %x)
  ret void
}
