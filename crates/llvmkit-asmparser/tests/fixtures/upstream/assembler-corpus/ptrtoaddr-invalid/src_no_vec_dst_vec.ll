













; RUN: not llvm-as %t/src_no_vec_dst_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_NO_VEC_DST_VEC %s --implicit-check-not="error:"
define <2 x i64> @bad(ptr %p) {
  %addr = ptrtoaddr ptr %p to <2 x i64>
  ; SRC_NO_VEC_DST_VEC: [[#@LINE-1]]:21: error: invalid cast opcode for cast from 'ptr' to '<2 x i64>'
  ret <2 x i64> %addr
}

