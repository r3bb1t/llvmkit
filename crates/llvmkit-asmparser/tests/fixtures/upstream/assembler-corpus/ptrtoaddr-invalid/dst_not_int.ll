





















; RUN: not llvm-as %t/dst_not_int.ll -o /dev/null 2>&1 | FileCheck -check-prefix=DST_NOT_INT %s --implicit-check-not="error:"
define float @bad(ptr %p) {
  %addr = ptrtoaddr ptr %p to float
  ; DST_NOT_INT: [[#@LINE-1]]:21: error: invalid cast opcode for cast from 'ptr' to 'float'
  ret float %addr
}

