; llvmkit-specific subset of llvm/test/Assembler/callbr.ll.
; Upstream uses @llvm.amdgcn.kill; llvmkit's intrinsic table does not model
; that target intrinsic yet, so this keeps the callbr successor structure with
; an ordinary declared callee.

declare void @callee(i1)

define void @test_kill(i1 %c) {
  callbr void @callee(i1 %c) to label %cont [label %kill]
kill:
  unreachable
cont:
  ret void
}
