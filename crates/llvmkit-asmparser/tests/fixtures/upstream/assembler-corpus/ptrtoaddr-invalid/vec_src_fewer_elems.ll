





















































; RUN: not llvm-as %t/vec_src_fewer_elems.ll -o /dev/null 2>&1 | FileCheck -check-prefix=VEC_SRC_FEWER_ELEMS %s --implicit-check-not="error:"
define <4 x i64> @bad(<2 x ptr> %p) {
  %addr = ptrtoaddr <2 x ptr> %p to <4 x i64>
  ; VEC_SRC_FEWER_ELEMS: [[#@LINE-1]]:21: error: invalid cast opcode for cast from '<2 x ptr>' to '<4 x i64>'
  ret <4 x i64> %addr
}

