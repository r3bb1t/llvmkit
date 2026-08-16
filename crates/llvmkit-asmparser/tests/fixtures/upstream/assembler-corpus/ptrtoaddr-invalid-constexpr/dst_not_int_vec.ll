



















; RUN: not llvm-as %t/dst_not_int_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=DST_NOT_INT_VEC %s --implicit-check-not="error:"
@g = global <2 x float> ptrtoaddr (<2 x ptr> <ptr @g, ptr @g> to <2 x float>)
; DST_NOT_INT_VEC: [[#@LINE-1]]:25: error: invalid cast opcode for cast from '<2 x ptr>' to '<2 x float>'
