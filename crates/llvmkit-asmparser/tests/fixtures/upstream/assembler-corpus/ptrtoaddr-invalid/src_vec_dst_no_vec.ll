





; RUN: not llvm-as %t/src_vec_dst_no_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_VEC_DST_NO_VEC %s --implicit-check-not="error:"
define i64 @bad(<2 x ptr> %p) {
  %addr = ptrtoaddr <2 x ptr> %p to i64
  ; SRC_VEC_DST_NO_VEC: [[#@LINE-1]]:21: error: invalid cast opcode for cast from '<2 x ptr>' to 'i64'
  ret i64 %addr
}

