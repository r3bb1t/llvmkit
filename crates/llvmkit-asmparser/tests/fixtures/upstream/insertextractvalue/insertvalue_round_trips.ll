; Source: llvm/test/Assembler/insertextractvalue.ll
; llvmkit-specific subset: upstream uses legacy typed pointers; this fixture
; keeps the nested load/extractvalue/insertvalue/store sequence with opaque ptr.

; CHECK:      @foo
; CHECK-NEXT: load
; CHECK-NEXT: extractvalue
; CHECK-NEXT: insertvalue
; CHECK-NEXT: store
; CHECK-NEXT: ret
define float @foo(ptr %p) nounwind {
  %t = load {{i32},{float, double}}, ptr %p
  %s = extractvalue {{i32},{float, double}} %t, 1, 0
  %r = insertvalue {{i32},{float, double}} %t, double 2.0, 1, 1
  store {{i32},{float, double}} %r, ptr %p
  ret float %s
}
