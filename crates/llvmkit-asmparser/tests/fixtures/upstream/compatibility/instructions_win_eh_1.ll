; Source: llvm/test/Bitcode/compatibility.ll -- `@instructions.win_eh.1`,
; verbatim, preceded by the `declare ccc void @f.ccc()` it calls (upstream
; declares it earlier in the same module). Upstream RUN line:
; `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s`.

declare ccc void @f.ccc()

define i32 @instructions.win_eh.1() personality i32 -3 {
entry:
  %arg1 = alloca i32
  %arg2 = alloca i32
  invoke void @f.ccc() to label %normal unwind label %catchswitch1
  invoke void @f.ccc() to label %normal unwind label %catchswitch2
  invoke void @f.ccc() to label %normal unwind label %catchswitch3

catchswitch1:
  %cs1 = catchswitch within none [label %catchpad1] unwind to caller

catchpad1:
  catchpad within %cs1 []
  br label %normal
  ; CHECK: catchpad within %cs1 []
  ; CHECK-NEXT: br label %normal

catchswitch2:
  %cs2 = catchswitch within none [label %catchpad2] unwind to caller

catchpad2:
  catchpad within %cs2 [ptr %arg1]
  br label %normal
  ; CHECK: catchpad within %cs2 [ptr %arg1]
  ; CHECK-NEXT: br label %normal

catchswitch3:
  %cs3 = catchswitch within none [label %catchpad3] unwind label %cleanuppad1

catchpad3:
  catchpad within %cs3 [ptr %arg1, ptr %arg2]
  br label %normal
  ; CHECK: catchpad within %cs3 [ptr %arg1, ptr %arg2]
  ; CHECK-NEXT: br label %normal

cleanuppad1:
  %clean.1 = cleanuppad within none []
  unreachable
  ; CHECK: %clean.1 = cleanuppad within none []
  ; CHECK-NEXT: unreachable

normal:
  ret i32 0
}
