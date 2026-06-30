; llvmkit-specific subset of llvm/test/Assembler/callbr.ll.
; Keeps the upstream @llvm.amdgcn.kill intrinsic callee while reducing the
; fixture to the successor structure exercised by llvmkit's callbr parser.

define void @test_kill(i1 %c) {
  callbr void @llvm.amdgcn.kill(i1 %c) to label %cont [label %kill]
kill:
  unreachable
cont:
  ret void
}
