















; RUN: not llvm-as %t/dst_not_int.ll -o /dev/null 2>&1 | FileCheck -check-prefix=DST_NOT_INT %s --implicit-check-not="error:"
@g = global float ptrtoaddr (ptr @g to float)
; DST_NOT_INT: [[#@LINE-1]]:19: error: invalid cast opcode for cast from 'ptr' to 'float'

