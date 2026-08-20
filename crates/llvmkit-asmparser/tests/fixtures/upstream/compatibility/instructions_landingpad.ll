; Source: llvm/test/Bitcode/compatibility.ll -- `@instructions.landingpad`,
; verbatim, preceded by the `declare void @llvm.donothing()` it invokes
; (upstream declares it earlier in the same module). Upstream RUN line:
; `llvm-as < %s | llvm-dis | llvm-as | llvm-dis | FileCheck %s`.
; The declaration line carries its own upstream CHECK
; (`declare void @llvm.donothing() #35`), which names a file-wide attribute
; group number and is not part of this excerpt.

declare void @llvm.donothing() nounwind readnone


define void @instructions.landingpad() personality i32 -2 {
  invoke void @llvm.donothing() to label %proceed unwind label %catch1
  invoke void @llvm.donothing() to label %proceed unwind label %catch2
  invoke void @llvm.donothing() to label %proceed unwind label %catch3
  invoke void @llvm.donothing() to label %proceed unwind label %catch4

catch1:
  landingpad i32
  ; CHECK: landingpad i32
             cleanup
             ; CHECK: cleanup
  br label %proceed

catch2:
  landingpad i32
  ; CHECK: landingpad i32
             cleanup
             ; CHECK: cleanup
             catch ptr null
             ; CHECK: catch ptr null
  br label %proceed

catch3:
  landingpad i32
  ; CHECK: landingpad i32
             cleanup
             ; CHECK: cleanup
             catch ptr null
             ; CHECK: catch ptr null
             catch ptr null
             ; CHECK: catch ptr null
  br label %proceed

catch4:
  landingpad i32
  ; CHECK: landingpad i32
             filter [2 x i32] zeroinitializer
             ; CHECK: filter [2 x i32] zeroinitializer
  br label %proceed

proceed:
  ret void
}
