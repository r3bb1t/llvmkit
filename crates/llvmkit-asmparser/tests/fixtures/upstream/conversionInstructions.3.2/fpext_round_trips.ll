; Source: llvm/test/Bitcode/conversionInstructions.3.2.ll

define void @fpext(float %src){
entry:
; CHECK: %res1 = fpext float %src to double
  %res1 = fpext float %src to double
  
  ret void
}
