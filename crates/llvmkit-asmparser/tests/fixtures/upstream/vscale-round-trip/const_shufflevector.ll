; Excerpted verbatim from llvm/test/Bitcode/vscale-round-trip.ll const_shufflevector cases.

target datalayout = "e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128"
target triple = "aarch64"

; CHECK-LABEL: define <vscale x 4 x i32> @const_shufflevector(
; CHECK: <vscale x 4 x i32> zeroinitializer

define <vscale x 4 x i32> @const_shufflevector() {
  ret <vscale x 4 x i32> shufflevector (<vscale x 4 x i32> zeroinitializer,
                                        <vscale x 4 x i32> undef,
                                        <vscale x 4 x i32> zeroinitializer)
}

; CHECK-LABEL: define <vscale x 4 x i32> @const_shufflevector_ex()
; CHECK: <vscale x 4 x i32> zeroinitializer

define <vscale x 4 x i32> @const_shufflevector_ex() {
  ret <vscale x 4 x i32> shufflevector (<vscale x 2 x i32> zeroinitializer,
                                        <vscale x 2 x i32> undef,
                                        <vscale x 4 x i32> zeroinitializer)
}
