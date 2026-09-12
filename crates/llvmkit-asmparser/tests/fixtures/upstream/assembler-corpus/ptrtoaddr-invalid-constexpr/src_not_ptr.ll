

























; RUN: not llvm-as %t/src_not_ptr.ll -o /dev/null 2>&1 | FileCheck -check-prefix=SRC_NOT_PTR %s --implicit-check-not="error:"
@g = global i64 ptrtoaddr (i32 1 to i64)
; SRC_NOT_PTR: [[#@LINE-1]]:17: error: invalid cast opcode for cast from 'i32' to 'i64'

