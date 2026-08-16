; llvmkit-specific subset of
; llvm/test/Transforms/PreISelIntrinsicLowering/protected-field-pointer.ll:
; the NOPAUTH-lowered call shape, which carries the "deactivation-symbol"
; operand bundle (registered as LLVMContext::OB_deactivation_symbol and
; spelled by knownBundleName in lib/IR/LLVMContext.cpp). Upstream's source
; lines call the @llvm.protected.field.ptr intrinsic, which llvmkit does not
; model; the bundle tag under test is identical on the lowered plain call.

@ds1 = external global i8

declare i64 @__emupac_autda(i64, i64)

define i64 @load_hw(i64 %val) {
  %auted = call i64 @__emupac_autda(i64 %val, i64 1) [ "deactivation-symbol"(ptr @ds1) ]
  ret i64 %auted
}
