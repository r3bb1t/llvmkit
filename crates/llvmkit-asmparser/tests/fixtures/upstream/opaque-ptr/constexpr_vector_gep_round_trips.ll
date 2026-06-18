; Copied from llvm/test/Assembler/opaque-ptr.ll::gep_constexpr_vec1.
; RUN: llvm-as < %s | llvm-dis | FileCheck %s

; CHECK: define <2 x ptr> @gep_constexpr_vec1(ptr %a)
; CHECK:     ret <2 x ptr> getelementptr (i16, ptr null, <2 x i32> <i32 3, i32 4>)
define <2 x ptr> @gep_constexpr_vec1(ptr %a) {
  ret <2 x ptr> getelementptr (i16, ptr null, <2 x i32> <i32 3, i32 4>)
}
