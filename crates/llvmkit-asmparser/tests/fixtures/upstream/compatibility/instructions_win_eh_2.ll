; Source: llvm/test/Bitcode/compatibility.ll -- `@instructions.win_eh.2`,
; verbatim, preceded by the `declare ccc void @f.ccc()` it calls (upstream
; declares it earlier in the same module). Upstream RUN line:
; `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s`.

declare ccc void @f.ccc()

define i32 @instructions.win_eh.2() personality i32 -4 {
entry:
  invoke void @f.ccc() to label %invoke.cont unwind label %catchswitch

invoke.cont:
  invoke void @f.ccc() to label %continue unwind label %cleanup

cleanup:
  %clean = cleanuppad within none []
  ; CHECK: %clean = cleanuppad within none []
  cleanupret from %clean unwind to caller
  ; CHECK: cleanupret from %clean unwind to caller

catchswitch:
  %cs = catchswitch within none [label %catchpad] unwind label %terminate

catchpad:
  %catch = catchpad within %cs []
  br label %body
  ; CHECK: %catch = catchpad within %cs []
  ; CHECK-NEXT: br label %body

body:
  invoke void @f.ccc() [ "funclet"(token %catch) ]
    to label %continue unwind label %terminate.inner
  catchret from %catch to label %return
  ; CHECK: catchret from %catch to label %return

return:
  ret i32 0

terminate.inner:
  cleanuppad within %catch []
  unreachable
  ; CHECK: cleanuppad within %catch []
  ; CHECK-NEXT: unreachable

terminate:
  cleanuppad within none []
  unreachable
  ; CHECK: cleanuppad within none []
  ; CHECK-NEXT: unreachable

continue:
  ret i32 0
}
