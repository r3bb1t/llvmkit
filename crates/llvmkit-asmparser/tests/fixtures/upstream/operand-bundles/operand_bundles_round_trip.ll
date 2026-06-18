; llvmkit-specific subset of llvm/test/Bitcode/operand-bundles.ll.
; Upstream covers typed-pointer loads, metadata bundles, and landingpad-heavy
; invoke variants; llvmkit keeps the call/invoke operand-bundle print shape
; that its parser models today.

declare void @callee0()

define void @f(i32 %x) {
entry:
  call void @callee0() [ "foo"(i32 42, i32 %x), "bar"() ]
  invoke void @callee0() [ "foo"(i32 %x) ] to label %ok unwind label %bad
ok:
  ret void
bad:
  unreachable
}
