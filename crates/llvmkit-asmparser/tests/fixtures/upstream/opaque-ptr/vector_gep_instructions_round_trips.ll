; Copied from llvm/test/Assembler/opaque-ptr.ll::gep_vec1 and ::gep_vec2.
; RUN: llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s

; CHECK: define <2 x ptr> @gep_vec1(ptr %a)
; CHECK:     %res = getelementptr i8, ptr %a, <2 x i32> <i32 1, i32 2>
; CHECK:     ret <2 x ptr> %res
define <2 x ptr> @gep_vec1(ptr %a) {
  %res = getelementptr i8, ptr %a, <2 x i32> <i32 1, i32 2>
  ret <2 x ptr> %res
}

; CHECK: define <2 x ptr> @gep_vec2(<2 x ptr> %a)
; CHECK:     %res = getelementptr i8, <2 x ptr> %a, i32 2
; CHECK:     ret <2 x ptr> %res
define <2 x ptr> @gep_vec2(<2 x ptr> %a) {
  %res = getelementptr i8, <2 x ptr> %a, i32 2
  ret <2 x ptr> %res
}
