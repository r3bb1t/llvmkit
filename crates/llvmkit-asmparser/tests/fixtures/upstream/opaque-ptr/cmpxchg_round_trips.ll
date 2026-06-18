; Source: llvm/test/Assembler/opaque-ptr.ll

; CHECK: define void @cmpxchg(ptr %p, i32 %a, i32 %b)
; CHECK:     %val_success = cmpxchg ptr %p, i32 %a, i32 %b acq_rel monotonic
; CHECK:     ret void
define void @cmpxchg(ptr %p, i32 %a, i32 %b) {
    %val_success = cmpxchg ptr %p, i32 %a, i32 %b acq_rel monotonic
    ret void
}
