; llvmkit-specific subset of llvm/test/Bindings/llvm-c/echo.ll @call_inline_asm.
; Upstream uses numbered values and tail calls; llvmkit's parser subset keeps the
; same inline-asm modifier spelling with named values and ordinary calls.

define i32 @call_inline_asm(i32 %x) {
entry:
  %intel = call i32 asm inteldialect unwind "mov $0, $1", "=r,r,~{dirflag},~{fpsr},~{flags}"(i32 %x)
  %att = call i32 asm alignstack unwind "mov $1, $0", "=r,r,~{dirflag},~{fpsr},~{flags}"(i32 %intel)
  ret i32 %att
}
