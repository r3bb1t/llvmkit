


































; RUN: not llvm-as %t/vec_src_fewer_elems.ll -o /dev/null 2>&1 | FileCheck -check-prefix=VEC_SRC_FEWER_ELEMS %s --implicit-check-not="error:"
@g = global <4 x i64> ptrtoaddr (<2 x ptr> <ptr @g, ptr @g> to <4 x i64>)
; VEC_SRC_FEWER_ELEMS: [[#@LINE-1]]:23: error: invalid cast opcode for cast from '<2 x ptr>' to '<4 x i64>'
