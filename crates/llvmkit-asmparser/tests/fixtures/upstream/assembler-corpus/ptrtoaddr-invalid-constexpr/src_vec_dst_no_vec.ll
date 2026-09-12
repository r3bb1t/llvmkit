





; RUN: not llvm-as %t/src_vec_dst_no_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_VEC_DST_NO_VEC %s --implicit-check-not="error:"
@g = global i64 ptrtoaddr (<2 x ptr> <ptr @g, ptr @g> to i64)
; SRC_VEC_DST_NO_VEC: [[#@LINE-1]]:17: error: invalid cast opcode for cast from '<2 x ptr>' to 'i64'

