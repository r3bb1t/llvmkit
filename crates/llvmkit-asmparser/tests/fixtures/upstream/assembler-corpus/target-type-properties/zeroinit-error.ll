define void @foo() {
  %val = freeze target("spirv.DeviceEvent") zeroinitializer
  %val2 = freeze target("unknown_target_type") zeroinitializer
; CHECK-ZEROINIT: error: invalid type for null constant
  ret void
}
