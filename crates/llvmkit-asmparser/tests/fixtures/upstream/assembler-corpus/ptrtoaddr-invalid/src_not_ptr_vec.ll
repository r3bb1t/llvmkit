













































; RUN: not llvm-as %t/src_not_ptr_vec.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_NOT_PTR_VEC %s --implicit-check-not="error:"
define <2 x i64> @bad(<2 x i32> %p) {
  %addr = ptrtoaddr <2 x i32> %p to <2 x i64>
  ; SRC_NOT_PTR_VEC: [[#@LINE-1]]:21: error: invalid cast opcode for cast from '<2 x i32>' to '<2 x i64>'
  ret <2 x i64> %addr
}

