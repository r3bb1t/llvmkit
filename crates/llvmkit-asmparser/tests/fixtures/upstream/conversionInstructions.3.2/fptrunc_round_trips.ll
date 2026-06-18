; Source: llvm/test/Bitcode/conversionInstructions.3.2.ll

define void @fptrunc(double %src){
entry:
; CHECK: %res1 = fptrunc double %src to float
  %res1 = fptrunc double %src to float
  
  ret void
}
