





























; RUN: not llvm-as %t/dst_not_int_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=DST_NOT_INT_VEC %s --implicit-check-not="error:"
define <2 x float> @bad(<2 x ptr> %p) {
  %addr = ptrtoaddr <2 x ptr> %p to <2 x float>
  ; DST_NOT_INT_VEC: [[#@LINE-1]]:21: error: invalid cast opcode for cast from '<2 x ptr>' to '<2 x float>'
  ret <2 x float> %addr
}

