









; RUN: not llvm-as %t/src_no_vec_dst_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_NO_VEC_DST_VEC %s --implicit-check-not="error:"
@g = global <2 x i64> ptrtoaddr (ptr @g to <2 x i64>)
; SRC_NO_VEC_DST_VEC: [[#@LINE-1]]:23: error: invalid cast opcode for cast from 'ptr' to '<2 x i64>'
