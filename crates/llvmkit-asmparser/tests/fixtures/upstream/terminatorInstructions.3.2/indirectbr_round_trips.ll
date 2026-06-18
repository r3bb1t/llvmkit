; Source: llvm/test/Bitcode/terminatorInstructions.3.2.ll
; llvmkit-specific subset: upstream 3.2 fixture uses legacy i8* spelling;
; this fixture keeps the same successor list with opaque ptr syntax.

define i32 @indirectbr(ptr %Addr){
entry:
; CHECK: indirectbr ptr %Addr, [label %bb1, label %bb2]
  indirectbr ptr %Addr, [ label %bb1, label %bb2 ]

  bb1:
  ret i32 1

  bb2:
  ret i32 0
}
