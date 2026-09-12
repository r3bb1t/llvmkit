






























; RUN: not llvm-as %t/src_not_ptr_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_NOT_PTR_VEC %s --implicit-check-not="error:"
@g = global <2 x i64> ptrtoaddr (<2 x i32> <i32 1, i32 2> to <2 x i64>)
; SRC_NOT_PTR_VEC: [[#@LINE-1]]:23: error: invalid cast opcode for cast from '<2 x i32>' to '<2 x i64>'

