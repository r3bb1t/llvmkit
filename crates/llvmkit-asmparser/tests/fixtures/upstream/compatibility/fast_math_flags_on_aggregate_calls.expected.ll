; ModuleID = 'fast_math_flags_on_aggregate_calls.ll'

declare [2 x float] @fmf_a2f32()

declare [2 x double] @fmf_a2f64()

declare [2 x <4 x double>] @fmf_a2v4f64()

define void @fastMathFlagsForArrayCalls([2 x float] %f, [2 x double] %d1, [2 x <4 x double>] %d2) {
  %call.fast = call fast [2 x float] @fmf_a2f32()
  %call.nsz.arcp = notail call nsz arcp [2 x double] @fmf_a2f64()
  %call.nnan.ninf = tail call nnan ninf fastcc [2 x <4 x double>] @fmf_a2v4f64()
  ret void
}

declare { float, float } @fmf_struct_f32()

declare { double, double, double } @fmf_struct_f64()

declare { <4 x double> } @fmf_struct_v4f64()

define void @fastMathFlagsForStructCalls() {
  %call.fast = call fast { float, float } @fmf_struct_f32()
  %call.nsz.arcp = notail call nsz arcp { double, double, double } @fmf_struct_f64()
  %call.nnan.ninf = tail call nnan ninf fastcc { <4 x double> } @fmf_struct_v4f64()
  ret void
}
